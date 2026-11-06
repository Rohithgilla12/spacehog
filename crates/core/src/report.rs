use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::human;
use crate::index::{Index, now};
use crate::model::{NodeFlags, NodeInfo, NodeKind, ScanId, ScanRecord};
use crate::query::{KindFilter, NodeFilter};
use crate::rules::{RuleDef, RuleSet, Safety};

/// A categorised node as every surface reports it.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub path: String,
    pub size: u64,
    pub size_human: String,
    pub apparent_size: u64,
    pub kind: NodeKind,
    pub bucket: String,
    pub bucket_label: String,
    pub rule_id: Option<String>,
    pub rule_label: Option<String>,
    pub safety: Safety,
    pub stale_days: Option<i64>,
    pub project_root: Option<String>,
    pub regen_hint: Option<String>,
    pub last_modified: i64,
    #[serde(skip)]
    pub node_id: i64,
}

impl Item {
    pub fn from_node(rules: &RuleSet, n: &NodeInfo) -> Self {
        let size = if n.claim {
            n.excl_disk.unwrap_or(n.disk_size)
        } else {
            n.disk_size
        };
        let bucket = n.bucket.clone().unwrap_or_else(|| "other".into());
        let rule = n.rule_id.as_deref().and_then(|id| rules.get(id));
        Self {
            path: n.path.clone(),
            size,
            size_human: human::bytes(size),
            apparent_size: if n.claim {
                n.excl_apparent.unwrap_or(n.apparent_size)
            } else {
                n.apparent_size
            },
            kind: n.kind,
            bucket_label: rules
                .bucket(&bucket)
                .map_or_else(|| bucket.clone(), |b| b.label.clone()),
            bucket,
            rule_id: n.rule_id.clone(),
            rule_label: rule.map(|r| r.def.label.clone()),
            safety: rules.effective_safety(&n.path, n.rule_id.as_deref(), n.safety),
            stale_days: n.stale_days,
            project_root: n.project_root.clone(),
            regen_hint: n
                .regen_hint
                .clone()
                .or_else(|| rule.and_then(|r| r.def.regen.clone())),
            last_modified: n.max_mtime,
            node_id: n.id,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TopQuery {
    pub limit: usize,
    /// A bucket id, a category, or a rule id.
    pub category: Option<String>,
    pub min_size: Option<u64>,
    pub stale_days: Option<i64>,
    pub safety: Option<Vec<Safety>>,
    pub under: Option<std::path::PathBuf>,
    pub kind: KindFilter,
}

/// Largest categorised items: claim roots ranked by the bytes they alone account for.
pub fn top(index: &Index, rules: &RuleSet, scan: ScanId, q: &TopQuery) -> Result<Vec<Item>> {
    let rule_ids =
        match &q.category {
            Some(c) => Some(rules.resolve_category(c).ok_or_else(|| {
                Error::Invalid(format!("unknown category, bucket, or rule: {c}"))
            })?),
            None => None,
        };
    let limit = if q.limit == 0 { 20 } else { q.limit };
    let filter = NodeFilter {
        claims_only: true,
        rule_ids,
        min_size: q.min_size,
        stale_days: q.stale_days,
        under: q.under.clone(),
        kind: q.kind,
        safety: q.safety.clone(),
        limit: limit * 2,
        ..Default::default()
    };
    let mut items: Vec<Item> = index
        .select(scan, &filter)?
        .iter()
        .map(|n| Item::from_node(rules, n))
        .filter(|i| q.safety.as_ref().is_none_or(|s| s.contains(&i.safety)))
        .collect();
    items.truncate(limit);
    Ok(items)
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketSummary {
    pub id: String,
    pub label: String,
    pub colour: String,
    pub bytes: u64,
    pub human: String,
    pub pct: f64,
    pub stale_bytes: u64,
    pub stale_human: String,
    pub items: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleSummary {
    pub rule_id: String,
    pub label: String,
    pub bucket: String,
    pub safety: Safety,
    pub bytes: u64,
    pub human: String,
    pub items: u64,
    pub stale_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub scan: ScanRecord,
    pub total: u64,
    pub total_human: String,
    pub stale_days: i64,
    pub stale_bytes: u64,
    pub stale_human: String,
    pub buckets: Vec<BucketSummary>,
    pub top_rules: Vec<RuleSummary>,
    pub rules: Vec<RuleSummary>,
}

pub fn summary(index: &Index, rules: &RuleSet, scan: ScanId, stale_days: i64) -> Result<Summary> {
    let record = index.scan(scan)?.ok_or(Error::NoScan)?;
    let root = index.root_node(scan)?.ok_or(Error::NoScan)?;
    let cutoff = now() - stale_days * 86_400;
    let mut stmt = index.connection().prepare_cached(
        "SELECT rule_id, bucket, SUM(excl_disk), COUNT(*),
                SUM(CASE WHEN stale_ref IS NOT NULL AND stale_ref <= ?2 THEN excl_disk ELSE 0 END)
         FROM nodes WHERE scan_id = ?1 AND claim = 1 AND (flags & ?3) = 0
         GROUP BY rule_id, bucket",
    )?;
    let rows: Vec<(String, String, i64, i64, i64)> = stmt
        .query_map(rusqlite::params![scan, cutoff, NodeFlags::DELETED], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<std::result::Result<_, _>>()?;

    let total = root.disk_size;
    let mut by_bucket: HashMap<String, (u64, u64, u64)> = HashMap::new();
    let mut rule_rows = Vec::new();
    let mut claimed = 0u64;
    for (rule_id, bucket, bytes, items, stale) in rows {
        let (bytes, items, stale) = (bytes as u64, items as u64, stale as u64);
        claimed += bytes;
        let e = by_bucket.entry(bucket.clone()).or_default();
        e.0 += bytes;
        e.1 += items;
        e.2 += stale;
        let rule = rules.get(&rule_id);
        rule_rows.push(RuleSummary {
            label: rule.map_or_else(|| rule_id.clone(), |r| r.def.label.clone()),
            safety: rule.map_or(Safety::Protected, |r| r.safety()),
            rule_id,
            bucket,
            bytes,
            human: human::bytes(bytes),
            items,
            stale_bytes: stale,
        });
    }
    let other = by_bucket.entry("other".into()).or_default();
    other.0 += total.saturating_sub(claimed);

    let mut buckets: Vec<BucketSummary> = by_bucket
        .into_iter()
        .filter(|(_, (b, _, _))| *b > 0)
        .map(|(id, (bytes, items, stale))| {
            let def = rules.bucket(&id);
            BucketSummary {
                label: def.map_or_else(|| id.clone(), |b| b.label.clone()),
                colour: def.map_or_else(|| "#6272a4".into(), |b| b.colour.clone()),
                pct: if total == 0 {
                    0.0
                } else {
                    (bytes as f64 / total as f64 * 1000.0).round() / 10.0
                },
                human: human::bytes(bytes),
                stale_human: human::bytes(stale),
                id,
                bytes,
                stale_bytes: stale,
                items,
            }
        })
        .collect();
    buckets.sort_by_key(|b| std::cmp::Reverse(b.bytes));
    rule_rows.sort_by_key(|r| std::cmp::Reverse(r.bytes));
    let stale_bytes = rule_rows.iter().map(|r| r.stale_bytes).sum();
    Ok(Summary {
        scan: record,
        total,
        total_human: human::bytes(total),
        stale_days,
        stale_bytes,
        stale_human: human::bytes(stale_bytes),
        buckets,
        top_rules: rule_rows.iter().take(10).cloned().collect(),
        rules: rule_rows,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    pub item: Item,
    pub node: NodeInfo,
    pub claimed_by: Option<String>,
    pub rule: Option<RuleDef>,
    pub staleness: String,
    pub safety_reason: String,
    pub children: Vec<Item>,
    pub last_scanned: Option<i64>,
}

pub fn explain(
    index: &Index,
    rules: &RuleSet,
    path: &Path,
    stale_days: i64,
) -> Result<Explanation> {
    let path = crate::engine::canonical(path);
    let node = index
        .find_in_latest(&path)?
        .ok_or_else(|| Error::NotIndexed(path.clone()))?;
    let claim = claim_of(index, &node)?;
    let rule = node.rule_id.as_deref().and_then(|id| rules.get(id));
    let item = Item::from_node(rules, &node);
    let staleness = match (&claim, claim.as_ref().and_then(|c| c.stale_days)) {
        (Some(c), Some(days)) => {
            let basis = match &c.project_root {
                Some(root)
                    if rule.is_some_and(|r| r.def.stale == crate::rules::StaleBasis::Project) =>
                {
                    format!("its project {root} (manifests, .git/HEAD, sibling files)")
                }
                _ => "the newest file inside it".to_string(),
            };
            let verdict = if days >= stale_days {
                "stale"
            } else {
                "active"
            };
            format!(
                "{verdict}: last activity {days} days ago, measured from {basis}; threshold {stale_days} days"
            )
        }
        (Some(_), None) => "not tracked: this rule never reports staleness".into(),
        (None, _) => "not tracked: no rule claims this path".into(),
    };
    let safety_reason = match (rule, &claim) {
        (Some(r), Some(c)) if c.id == node.id => {
            format!(
                "{} via rule `{}` ({})",
                item.safety.as_str(),
                r.id,
                r.def.label
            )
        }
        (Some(r), Some(c)) => format!(
            "{} inherited from {} matched by rule `{}`",
            item.safety.as_str(),
            c.path,
            r.id
        ),
        _ => "protected: unclaimed paths are never cleaned".into(),
    };
    let children = if node.kind == NodeKind::Dir {
        index
            .children(node.id, 10)?
            .iter()
            .map(|c| Item::from_node(rules, c))
            .collect()
    } else {
        Vec::new()
    };
    let last_scanned = index.scan(node.scan_id)?.and_then(|s| s.finished_at);
    Ok(Explanation {
        item,
        claimed_by: claim.as_ref().map(|c| c.path.clone()),
        rule: rule.map(|r| r.def.clone()),
        node,
        staleness,
        safety_reason,
        children,
        last_scanned,
    })
}

/// The claim that governs a node: itself if it is one, else its nearest claimed ancestor.
pub fn claim_of(index: &Index, node: &NodeInfo) -> Result<Option<NodeInfo>> {
    if node.claim {
        return Ok(Some(node.clone()));
    }
    if node.rule_id.is_none() {
        return Ok(None);
    }
    let mut cur = node.parent_id;
    while let Some(id) = cur {
        let Some(n) = index.node(id)? else { break };
        if n.claim {
            return Ok(Some(n));
        }
        cur = n.parent_id;
    }
    Ok(None)
}
