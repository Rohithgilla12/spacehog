use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::Serialize;

use crate::config::{Config, DeleteMode};
use crate::error::{Error, Result};
use crate::human;
use crate::index::{Index, now};
use crate::model::NodeKind;
use crate::proposals::{Actor, Proposal, ProposalItem, ProposalStatus, new_journal_id, refusal};
use crate::rules::RuleSet;
use crate::scan::{Progress, ScanOptions};

/// Where trashed items go. macOS gets a rename into `~/.Trash` because the
/// system trash API cannot list or restore items there, which `undo` needs.
#[derive(Debug, Clone)]
pub enum Trash {
    Rename(PathBuf),
    System,
}

impl Trash {
    /// `SPACEHOG_TRASH_DIR` redirects trashing to a directory, for tests.
    pub fn platform_default() -> Self {
        if let Some(dir) = std::env::var_os("SPACEHOG_TRASH_DIR").filter(|v| !v.is_empty()) {
            return Trash::Rename(dir.into());
        }
        if cfg!(target_os = "macos") {
            Trash::Rename(crate::paths::home_dir().join(".Trash"))
        } else {
            Trash::System
        }
    }

    /// Moves `path` to the trash and returns where it landed, when known.
    fn put(&self, path: &Path, tag: &str) -> std::io::Result<Option<PathBuf>> {
        match self {
            Trash::Rename(dir) => {
                fs::create_dir_all(dir)?;
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut dest = dir.join(&name);
                if fs::symlink_metadata(&dest).is_ok() {
                    dest = dir.join(format!("{name} (spacehog {tag})"));
                }
                match fs::rename(path, &dest) {
                    Ok(()) => Ok(Some(dest)),
                    Err(e) if e.raw_os_error() == Some(libc::EXDEV) => Trash::System.put(path, tag),
                    Err(e) => Err(e),
                }
            }
            Trash::System => trash::delete(path)
                .map(|()| None)
                .map_err(std::io::Error::other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Trashed,
    Deleted,
    Skipped,
    Failed,
    Restored,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Trashed => "trashed",
            Outcome::Deleted => "deleted",
            Outcome::Skipped => "skipped",
            Outcome::Failed => "failed",
            Outcome::Restored => "restored",
        }
    }

    pub fn freed(self) -> bool {
        matches!(self, Outcome::Trashed | Outcome::Deleted)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemResult {
    pub path: String,
    pub size: u64,
    pub size_human: String,
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub trashed_to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionReport {
    pub proposal_id: String,
    pub journal_id: String,
    pub mode: DeleteMode,
    pub status: ProposalStatus,
    pub bytes_freed: u64,
    pub bytes_freed_human: String,
    pub results: Vec<ItemResult>,
}

pub struct ExecuteRequest<'a> {
    pub proposal_id: &'a str,
    pub confirm: &'a str,
    pub actor: Actor,
}

/// Decides whether `actor` may execute `p` right now.
fn authorise(p: &Proposal, config: &Config, req: &ExecuteRequest<'_>) -> Result<()> {
    if req.confirm.trim() != p.total_human {
        return Err(Error::Refused(format!(
            "confirm must equal the proposal total \"{}\"",
            p.total_human
        )));
    }
    let auto = p.mode == DeleteMode::Trash
        && p.safety_classes()
            .iter()
            .all(|s| config.mcp.auto_approve.contains(s));
    match (req.actor, p.status) {
        (_, ProposalStatus::Approved) => Ok(()),
        (Actor::User, ProposalStatus::Pending) => Ok(()),
        (Actor::Agent, ProposalStatus::Pending) if auto => Ok(()),
        (Actor::Agent, ProposalStatus::Pending) => Err(Error::Refused(
            "proposal is pending approval; ask the user to approve it in the spacehog TUI or with `spacehog proposals approve`"
                .into(),
        )),
        (_, s) => Err(Error::Refused(format!("proposal is {}", s.as_str()))),
    }
}

/// Recomputes the apparent size the scanner recorded, so a changed tree is caught.
fn current_apparent(path: &Path, meta: &fs::Metadata) -> std::io::Result<u64> {
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut opts = ScanOptions::new(path);
    opts.excludes.clear();
    let tree = crate::scan::scan(&opts, &Progress::default())?;
    Ok(tree.nodes[0].apparent)
}

/// Re-checks one item against the disk immediately before deleting it.
fn recheck(item: &ProposalItem, apparent: u64) -> std::result::Result<fs::Metadata, String> {
    let path = Path::new(&item.path);
    let meta = fs::symlink_metadata(path).map_err(|_| "no longer exists".to_string())?;
    if meta.ino() != item.inode || meta.dev() != item.dev {
        return Err("replaced since it was indexed (inode changed); rescan".into());
    }
    let was_link = item.kind == NodeKind::Symlink;
    if meta.file_type().is_symlink() != was_link {
        return Err("changed type since it was indexed; rescan".into());
    }
    let now = current_apparent(path, &meta).map_err(|e| format!("could not re-measure: {e}"))?;
    if now != apparent {
        return Err(format!(
            "changed since it was indexed ({} then, {} now); rescan",
            human::bytes(apparent),
            human::bytes(now)
        ));
    }
    Ok(meta)
}

impl Index {
    pub fn execute(
        &self,
        rules: &RuleSet,
        config: &Config,
        trash: &Trash,
        req: &ExecuteRequest<'_>,
    ) -> Result<ExecutionReport> {
        let p = self
            .proposal(req.proposal_id)?
            .ok_or_else(|| Error::Invalid(format!("no proposal {}", req.proposal_id)))?;
        authorise(&p, config, req)?;
        if p.items.len() > config.cleanup.max_items {
            return Err(Error::Refused(format!(
                "more than max_items ({})",
                config.cleanup.max_items
            )));
        }
        if p.total_bytes > config.cleanup.max_bytes_per_execution {
            return Err(Error::Refused(format!(
                "more than max_bytes_per_execution ({})",
                human::bytes(config.cleanup.max_bytes_per_execution)
            )));
        }
        let journal_id = new_journal_id();
        let mut results = Vec::with_capacity(p.items.len());
        let mut journal = JournalWriter::open(&self.conn)?;
        for item in &p.items {
            let result = self.execute_item(rules, config, trash, req.actor, &p, item, &journal_id);
            journal.record(&journal_id, &p, item, &result)?;
            results.push(result);
        }
        let freed: u64 = results
            .iter()
            .filter(|r| r.outcome.freed())
            .map(|r| r.size)
            .sum();
        let status = if results.iter().any(|r| r.outcome.freed()) {
            ProposalStatus::Executed
        } else {
            ProposalStatus::Failed
        };
        self.transition(
            &p.id,
            &[ProposalStatus::Pending, ProposalStatus::Approved],
            status,
            None,
            Some(&journal_id),
        )?;
        Ok(ExecutionReport {
            proposal_id: p.id,
            journal_id,
            mode: p.mode,
            status,
            bytes_freed: freed,
            bytes_freed_human: human::bytes(freed),
            results,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_item(
        &self,
        rules: &RuleSet,
        config: &Config,
        trash: &Trash,
        actor: Actor,
        p: &Proposal,
        item: &ProposalItem,
        journal_id: &str,
    ) -> ItemResult {
        let mut result = ItemResult {
            path: item.path.clone(),
            size: item.size,
            size_human: item.size_human.clone(),
            outcome: Outcome::Skipped,
            reason: None,
            trashed_to: None,
        };
        let path = Path::new(&item.path);
        match refusal(self, rules, config, actor, path) {
            Ok(Ok(_)) => {}
            Ok(Err(reason)) => {
                result.reason = Some(reason);
                return result;
            }
            Err(e) => {
                result.outcome = Outcome::Failed;
                result.reason = Some(e.to_string());
                return result;
            }
        }
        let Some(node) = item.node_id.and_then(|id| self.node(id).ok().flatten()) else {
            result.reason = Some("no longer in the index; rescan".into());
            return result;
        };
        let meta = match recheck(item, node.apparent_size) {
            Ok(m) => m,
            Err(reason) => {
                result.reason = Some(reason);
                return result;
            }
        };
        let done = match p.mode {
            DeleteMode::Trash => trash.put(path, journal_id).map(|to| {
                result.trashed_to = to.map(|t| t.to_string_lossy().into_owned());
                Outcome::Trashed
            }),
            DeleteMode::Permanent if meta.is_dir() => {
                fs::remove_dir_all(path).map(|()| Outcome::Deleted)
            }
            DeleteMode::Permanent => fs::remove_file(path).map(|()| Outcome::Deleted),
        };
        match done {
            Ok(outcome) => {
                result.outcome = outcome;
                if let Err(e) = self.mark_deleted(node.id) {
                    tracing::warn!("index not updated for {}: {e}", item.path);
                }
            }
            Err(e) => {
                result.outcome = Outcome::Failed;
                result.reason = Some(e.to_string());
            }
        }
        result
    }

    /// Moves trashed items from a journal back to where they were.
    pub fn undo(&self, journal_id: &str) -> Result<Vec<ItemResult>> {
        let rows: Vec<(String, i64, Option<String>, Option<i64>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT j.path, j.size, j.trashed_to, n.id FROM journal j
                 LEFT JOIN proposal_items i ON i.proposal_id = j.proposal_id AND i.path = j.path
                 LEFT JOIN nodes n ON n.id = i.node_id
                 WHERE j.journal_id = ?1 AND j.outcome = 'trashed'",
            )?;
            stmt.query_map([journal_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<_, _>>()?
        };
        if rows.is_empty() {
            return Err(Error::Invalid(format!(
                "journal {journal_id} has no trashed items to restore"
            )));
        }
        let mut journal = JournalWriter::open(&self.conn)?;
        let mut out = Vec::new();
        for (path, size, trashed_to, node_id) in rows {
            let size = size as u64;
            let mut r = ItemResult {
                path: path.clone(),
                size,
                size_human: human::bytes(size),
                outcome: Outcome::Failed,
                reason: None,
                trashed_to: trashed_to.clone(),
            };
            match restore(&path, trashed_to.as_deref()) {
                Ok(()) => {
                    r.outcome = Outcome::Restored;
                    if let Some(id) = node_id {
                        let _ = self.mark_restored(id);
                    }
                }
                Err(e) => r.reason = Some(e),
            }
            journal.record_restore(journal_id, &r)?;
            out.push(r);
        }
        Ok(out)
    }

    pub fn journal(&self, journal_id: &str) -> Result<Vec<serde_json::Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, size, inode, mode, outcome, error, trashed_to, ts, proposal_id, agent
             FROM journal WHERE journal_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([journal_id], |r| {
            Ok(serde_json::json!({
                "path": r.get::<_, String>(0)?,
                "size": r.get::<_, i64>(1)?,
                "inode": r.get::<_, i64>(2)?,
                "mode": r.get::<_, String>(3)?,
                "outcome": r.get::<_, String>(4)?,
                "error": r.get::<_, Option<String>>(5)?,
                "trashed_to": r.get::<_, Option<String>>(6)?,
                "ts": r.get::<_, i64>(7)?,
                "proposal_id": r.get::<_, Option<String>>(8)?,
                "agent": r.get::<_, Option<String>>(9)?,
            }))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn restore(original: &str, trashed_to: Option<&str>) -> std::result::Result<(), String> {
    if fs::symlink_metadata(original).is_ok() {
        return Err("something already exists at the original path".into());
    }
    if let Some(from) = trashed_to {
        if let Some(parent) = Path::new(original).parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        return fs::rename(from, original).map_err(|e| e.to_string());
    }
    restore_system(original)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn restore_system(original: &str) -> std::result::Result<(), String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    let mut matches: Vec<_> = items
        .into_iter()
        .filter(|i| i.original_path() == Path::new(original))
        .collect();
    matches.sort_by_key(|i| std::cmp::Reverse(i.time_deleted));
    let newest = matches
        .into_iter()
        .next()
        .ok_or("not found in the system trash")?;
    trash::os_limited::restore_all([newest]).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn restore_system(_original: &str) -> std::result::Result<(), String> {
    Err("moved to the Trash by Finder; use Put Back in Finder".into())
}

/// Appends to the `journal` table and to `journal.jsonl` in the state dir.
struct JournalWriter<'a> {
    conn: &'a rusqlite::Connection,
    file: Option<fs::File>,
}

impl<'a> JournalWriter<'a> {
    fn open(conn: &'a rusqlite::Connection) -> Result<Self> {
        let dir = crate::paths::state_dir();
        let file = fs::create_dir_all(&dir)
            .and_then(|()| {
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("journal.jsonl"))
            })
            .map_err(|e| tracing::warn!("journal.jsonl unavailable: {e}"))
            .ok();
        Ok(Self { conn, file })
    }

    fn write(&mut self, row: serde_json::Value) -> Result<()> {
        self.conn.execute(
            "INSERT INTO journal (journal_id, proposal_id, agent, path, size, inode, mode, outcome, error, ts,
                trashed_to)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                row["journal_id"].as_str(),
                row["proposal_id"].as_str(),
                row["agent"].as_str(),
                row["path"].as_str(),
                row["size"].as_i64(),
                row["inode"].as_i64().unwrap_or(0),
                row["mode"].as_str(),
                row["outcome"].as_str(),
                row["error"].as_str(),
                row["ts"].as_i64(),
                row["trashed_to"].as_str(),
            ],
        )?;
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{row}");
        }
        Ok(())
    }

    fn record(
        &mut self,
        journal_id: &str,
        p: &Proposal,
        item: &ProposalItem,
        r: &ItemResult,
    ) -> Result<()> {
        self.write(serde_json::json!({
            "journal_id": journal_id,
            "proposal_id": p.id,
            "agent": p.agent,
            "path": item.path,
            "size": item.size,
            "inode": item.inode,
            "mode": p.mode.as_str(),
            "outcome": r.outcome.as_str(),
            "error": r.reason,
            "ts": now(),
            "trashed_to": r.trashed_to,
        }))
    }

    fn record_restore(&mut self, journal_id: &str, r: &ItemResult) -> Result<()> {
        self.write(serde_json::json!({
            "journal_id": journal_id,
            "path": r.path,
            "size": r.size,
            "mode": "trash",
            "outcome": r.outcome.as_str(),
            "error": r.reason,
            "ts": now(),
            "trashed_to": r.trashed_to,
        }))
    }
}
