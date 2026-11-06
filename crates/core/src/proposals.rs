use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::config::{Config, DeleteMode};
use crate::error::{Error, Result};
use crate::human;
use crate::index::{Index, now};
use crate::model::{NodeFlags, NodeKind};
use crate::report::{Item, claim_of};
use crate::rules::{RuleSet, Safety};

/// Review items an agent may propose without `mcp.allow_review` must be idle this long.
pub const AGENT_REVIEW_MIN_STALE_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
    Executed,
    Failed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Approved => "approved",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Executed => "executed",
            ProposalStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(ProposalStatus::Pending),
            "approved" => Some(ProposalStatus::Approved),
            "rejected" => Some(ProposalStatus::Rejected),
            "executed" => Some(ProposalStatus::Executed),
            "failed" => Some(ProposalStatus::Failed),
            _ => None,
        }
    }
}

/// Who is asking. Agents face stricter rules for `review` items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Agent,
    User,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProposeRequest {
    pub paths: Vec<PathBuf>,
    pub rationale: String,
    pub agent: Option<String>,
    pub mode: Option<DeleteMode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rejection {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposeResult {
    pub proposal_id: Option<String>,
    pub accepted: Vec<Item>,
    pub rejected: Vec<Rejection>,
    pub total_bytes: u64,
    pub total_human: String,
    pub requires_approval: bool,
    pub mode: DeleteMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposalItem {
    pub path: String,
    pub node_id: Option<i64>,
    pub size: u64,
    pub size_human: String,
    pub inode: u64,
    pub dev: u64,
    pub kind: NodeKind,
    pub bucket: Option<String>,
    pub rule_id: Option<String>,
    pub safety: Safety,
    pub stale_days: Option<i64>,
    pub regen_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub scan_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub agent: String,
    pub rationale: String,
    pub mode: DeleteMode,
    pub status: ProposalStatus,
    pub total_bytes: u64,
    pub total_human: String,
    pub reason: Option<String>,
    pub journal_id: Option<String>,
    pub items: Vec<ProposalItem>,
}

impl Proposal {
    pub fn safety_classes(&self) -> HashSet<Safety> {
        self.items.iter().map(|i| i.safety).collect()
    }
}

fn new_id(prefix: &str) -> String {
    let nanos = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as u64;
    let mix = nanos ^ ((std::process::id() as u64) << 40) ^ nanos.rotate_left(17);
    format!("{prefix}{:08x}", mix as u32)
}

pub fn new_journal_id() -> String {
    new_id("j")
}

/// Why `path` may not be cleaned, or None when it may. Shared by proposal
/// validation and the executor so every surface enforces the same rules.
pub fn refusal(
    index: &Index,
    rules: &RuleSet,
    config: &Config,
    actor: Actor,
    path: &Path,
) -> Result<std::result::Result<Item, String>> {
    let Some(node) = index.find_in_latest(path)? else {
        return Ok(Err("not in the index; rescan first".into()));
    };
    if node.flags.has(NodeFlags::DELETED) {
        return Ok(Err("already deleted".into()));
    }
    let scan = index.scan(node.scan_id)?.ok_or(Error::NoScan)?;
    let root = Path::new(&scan.root);
    let home = rules.home.as_path();
    if path == root || root.starts_with(path) {
        return Ok(Err("is the scan root or one of its ancestors".into()));
    }
    if path == home || home.starts_with(path) {
        return Ok(Err("is your home directory or one of its ancestors".into()));
    }
    if node.kind == NodeKind::Mount || node.flags.has(NodeFlags::MOUNT) {
        return Ok(Err("is a mount point".into()));
    }
    if node.flags.has(NodeFlags::ERROR) {
        return Ok(Err("could not be read during the scan".into()));
    }
    let item = Item::from_node(rules, &node);
    let vcs_aware = item.bucket == "vcs";
    if node.kind == NodeKind::Dir
        && !vcs_aware
        && index.find(node.scan_id, &path.join(".git"))?.is_some()
    {
        return Ok(Err(
            "contains a .git directory, so it is a project, not a cache".into(),
        ));
    }
    match item.safety {
        Safety::Protected => {
            return Ok(Err(format!(
                "protected ({})",
                item.rule_label
                    .clone()
                    .unwrap_or_else(|| "not claimed by any rule".into())
            )));
        }
        Safety::Review if actor == Actor::Agent && !config.mcp.allow_review => {
            let days = claim_of(index, &node)?.and_then(|c| c.stale_days);
            if days.is_none_or(|d| d < AGENT_REVIEW_MIN_STALE_DAYS) {
                return Ok(Err(format!(
                    "review item idle {} days; agents need {AGENT_REVIEW_MIN_STALE_DAYS}+ or mcp.allow_review",
                    days.map_or("unknown".into(), |d| d.to_string())
                )));
            }
        }
        _ => {}
    }
    Ok(Ok(item))
}

impl Index {
    pub fn propose(
        &self,
        rules: &RuleSet,
        config: &Config,
        actor: Actor,
        req: &ProposeRequest,
    ) -> Result<ProposeResult> {
        let mode = req.mode.unwrap_or(config.cleanup.mode);
        let mut accepted: Vec<Item> = Vec::new();
        let mut rejected = Vec::new();
        let mut paths: Vec<PathBuf> = req
            .paths
            .iter()
            .map(|p| crate::engine::canonical(p))
            .collect();
        paths.sort();
        paths.dedup();
        let mut total = 0u64;
        for path in paths {
            let shown = path.to_string_lossy().into_owned();
            if let Some(outer) = accepted.iter().find(|a| path.starts_with(&a.path)) {
                rejected.push(Rejection {
                    path: shown,
                    reason: format!("already covered by {}", outer.path),
                });
                continue;
            }
            match refusal(self, rules, config, actor, &path)? {
                Err(reason) => rejected.push(Rejection {
                    path: shown,
                    reason,
                }),
                Ok(item) => {
                    if accepted.len() >= config.cleanup.max_items {
                        rejected.push(Rejection {
                            path: shown,
                            reason: format!("exceeds max_items ({})", config.cleanup.max_items),
                        });
                    } else if total + item.size > config.cleanup.max_bytes_per_execution {
                        rejected.push(Rejection {
                            path: shown,
                            reason: format!(
                                "exceeds max_bytes_per_execution ({})",
                                human::bytes(config.cleanup.max_bytes_per_execution)
                            ),
                        });
                    } else {
                        total += item.size;
                        accepted.push(item);
                    }
                }
            }
        }
        let requires_approval = mode == DeleteMode::Permanent
            || accepted
                .iter()
                .any(|i| !config.mcp.auto_approve.contains(&i.safety));
        let mut result = ProposeResult {
            proposal_id: None,
            accepted,
            rejected,
            total_bytes: total,
            total_human: human::bytes(total),
            requires_approval,
            mode,
        };
        if result.accepted.is_empty() {
            return Ok(result);
        }
        let id = new_id("p");
        let agent = req.agent.clone().unwrap_or_else(|| match actor {
            Actor::Agent => "agent".into(),
            Actor::User => "you".into(),
        });
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO proposals (id, scan_id, created_at, updated_at, agent, rationale, mode, status,
                total_bytes, total_human)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, 'pending', ?7, ?8)",
            params![
                id,
                self.latest_scan(None)?.map(|s| s.id),
                now(),
                agent,
                req.rationale,
                mode.as_str(),
                total as i64,
                result.total_human
            ],
        )?;
        for item in &result.accepted {
            let node = self
                .node(item.node_id)?
                .ok_or_else(|| Error::NotIndexed(item.path.clone().into()))?;
            tx.execute(
                "INSERT INTO proposal_items (proposal_id, path, node_id, size, inode, dev, kind, bucket,
                    rule_id, safety, stale_days, regen_hint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id,
                    item.path,
                    item.node_id,
                    item.size as i64,
                    node.inode as i64,
                    node.dev as i64,
                    node.kind.as_i64(),
                    item.bucket,
                    item.rule_id,
                    item.safety.as_str(),
                    item.stale_days,
                    item.regen_hint
                ],
            )?;
        }
        tx.commit()?;
        result.proposal_id = Some(id);
        Ok(result)
    }

    pub fn proposal(&self, id: &str) -> Result<Option<Proposal>> {
        let row = self
            .conn
            .query_row(
                &format!("{PROPOSAL_SELECT} WHERE id = ?1"),
                [id],
                proposal_from_row,
            )
            .optional()?;
        match row {
            Some(mut p) => {
                p.items = self.proposal_items(&p.id)?;
                Ok(Some(p))
            }
            None => Ok(None),
        }
    }

    pub fn proposals(&self, status: Option<ProposalStatus>) -> Result<Vec<Proposal>> {
        let mut out: Vec<Proposal> = match status {
            Some(s) => {
                let mut stmt = self.conn.prepare(&format!(
                    "{PROPOSAL_SELECT} WHERE status = ?1 ORDER BY created_at DESC"
                ))?;
                stmt.query_map([s.as_str()], proposal_from_row)?
                    .collect::<std::result::Result<_, _>>()?
            }
            None => {
                let mut stmt = self.conn.prepare(&format!(
                    "{PROPOSAL_SELECT} ORDER BY created_at DESC LIMIT 200"
                ))?;
                stmt.query_map([], proposal_from_row)?
                    .collect::<std::result::Result<_, _>>()?
            }
        };
        for p in &mut out {
            p.items = self.proposal_items(&p.id)?;
        }
        Ok(out)
    }

    fn proposal_items(&self, id: &str) -> Result<Vec<ProposalItem>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, node_id, size, inode, dev, kind, bucket, rule_id, safety, stale_days, regen_hint
             FROM proposal_items WHERE proposal_id = ?1 ORDER BY size DESC",
        )?;
        let rows = stmt.query_map([id], |r| {
            let size = r.get::<_, i64>(2)? as u64;
            Ok(ProposalItem {
                path: r.get(0)?,
                node_id: r.get(1)?,
                size,
                size_human: human::bytes(size),
                inode: r.get::<_, i64>(3)? as u64,
                dev: r.get::<_, i64>(4)? as u64,
                kind: NodeKind::from_i64(r.get(5)?),
                bucket: r.get(6)?,
                rule_id: r.get(7)?,
                safety: Safety::parse(&r.get::<_, String>(8)?).unwrap_or(Safety::Protected),
                stale_days: r.get(9)?,
                regen_hint: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Moves a proposal from `from` to `to`, failing if it is not in `from`.
    pub fn transition(
        &self,
        id: &str,
        from: &[ProposalStatus],
        to: ProposalStatus,
        reason: Option<&str>,
        journal_id: Option<&str>,
    ) -> Result<()> {
        let current = self
            .proposal(id)?
            .ok_or_else(|| Error::Invalid(format!("no proposal {id}")))?
            .status;
        if !from.contains(&current) {
            return Err(Error::Invalid(format!(
                "proposal {id} is {}, cannot become {}",
                current.as_str(),
                to.as_str()
            )));
        }
        self.conn.execute(
            "UPDATE proposals SET status = ?2, updated_at = ?3, reason = COALESCE(?4, reason),
                journal_id = COALESCE(?5, journal_id) WHERE id = ?1",
            params![id, to.as_str(), now(), reason, journal_id],
        )?;
        Ok(())
    }

    pub fn approve(&self, id: &str) -> Result<()> {
        self.transition(
            id,
            &[ProposalStatus::Pending],
            ProposalStatus::Approved,
            None,
            None,
        )
    }

    pub fn reject(&self, id: &str, reason: &str) -> Result<()> {
        self.transition(
            id,
            &[ProposalStatus::Pending, ProposalStatus::Approved],
            ProposalStatus::Rejected,
            Some(reason),
            None,
        )
    }

    pub fn pending_summary(&self) -> Result<(u64, u64)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(total_bytes), 0) FROM proposals WHERE status = 'pending'",
            [],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )?)
    }
}

const PROPOSAL_SELECT: &str =
    "SELECT id, scan_id, created_at, updated_at, agent, rationale, mode, status,
    total_bytes, total_human, reason, journal_id FROM proposals";

fn proposal_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    Ok(Proposal {
        id: r.get(0)?,
        scan_id: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
        agent: r.get(4)?,
        rationale: r.get(5)?,
        mode: DeleteMode::parse(&r.get::<_, String>(6)?).unwrap_or_default(),
        status: ProposalStatus::parse(&r.get::<_, String>(7)?).unwrap_or(ProposalStatus::Failed),
        total_bytes: r.get::<_, i64>(8)? as u64,
        total_human: r.get(9)?,
        reason: r.get(10)?,
        journal_id: r.get(11)?,
        items: Vec::new(),
    })
}
