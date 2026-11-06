use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use spacehog_core::Index;
use spacehog_core::categorise::Categoriser;
use spacehog_core::config::Config;
use spacehog_core::engine::{ScanJob, ScanRequest, spawn_scan};
use spacehog_core::model::{NodeFlags, NodeId, NodeInfo, NodeKind, ScanRecord, SizeMode};
use spacehog_core::report::{self, Item, TopQuery};
use spacehog_core::rules::{RuleSet, Safety};
use spacehog_core::status::{Volume, volume_for};

/// Everything the TUI reads from: the index, the rules, the config, and
/// where a new scan writes.
pub struct Ctx {
    pub index: Index,
    pub rules: Arc<RuleSet>,
    pub config: Config,
    pub db: PathBuf,
    pub lock_dir: PathBuf,
    pub home: PathBuf,
}

impl Ctx {
    pub fn load() -> Result<Self> {
        let config = Config::load()?;
        let home = spacehog_core::paths::home_dir();
        let rules = RuleSet::load(Some(&config.rules_path()), &home)?;
        let db = spacehog_core::paths::db_path();
        Ok(Self {
            index: Index::open(&db)?,
            rules: Arc::new(rules),
            config,
            db,
            lock_dir: spacehog_core::paths::data_dir(),
            home,
        })
    }

    pub fn spawn_scan(&self, root: &Path) -> Result<ScanJob> {
        let mut req = ScanRequest::new(root);
        req.db = self.db.clone();
        req.lock_dir = self.lock_dir.clone();
        req.keep = self.config.keep_scans;
        req.options.one_file_system = self.config.one_file_system;
        req.labeler = Some(Arc::new(Categoriser::new(self.rules.clone())));
        Ok(spawn_scan(req)?)
    }

    /// `~` for paths under home, so rows spend their width on the interesting part.
    pub fn tilde(&self, path: &str) -> String {
        match Path::new(path).strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.to_string(),
        }
    }
}

/// The session-wide view switches every query honours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toggles {
    pub size: SizeMode,
    pub stale_only: bool,
    pub hidden: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            size: SizeMode::Disk,
            stale_only: false,
            hidden: true,
        }
    }
}

/// One finished scan: its record, root node, and the volume it lives on.
pub struct Snapshot {
    pub scan: ScanRecord,
    pub root: NodeInfo,
    pub volume: Option<Volume>,
}

impl Snapshot {
    pub fn latest(index: &Index, root: Option<&Path>) -> Result<Option<Self>> {
        let Some(scan) = index.latest_scan(root)? else {
            return Ok(None);
        };
        let Some(root) = index.root_node(scan.id)? else {
            return Ok(None);
        };
        let volume = volume_for(Path::new(&root.path));
        Ok(Some(Self { scan, root, volume }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BucketSlice {
    pub id: String,
    pub label: String,
    pub colour: String,
    pub bytes: u64,
}

/// Bytes per rollup bucket. `report::summary` only reports disk bytes and
/// has no stale filter, so this runs its own grouping over `excl_*`.
pub fn buckets(ctx: &Ctx, snap: &Snapshot, t: Toggles) -> Result<Vec<BucketSlice>> {
    let col = match t.size {
        SizeMode::Disk => "excl_disk",
        SizeMode::Apparent => "excl_apparent",
    };
    let cutoff = t
        .stale_only
        .then(|| spacehog_core::index::now() - ctx.config.stale_days * 86_400);
    let sql = format!(
        "SELECT COALESCE(bucket, 'other'), SUM({col}) FROM nodes
         WHERE scan_id = ?1 AND claim = 1 AND (flags & ?2) = 0
           AND (?3 IS NULL OR (stale_ref IS NOT NULL AND stale_ref <= ?3))
         GROUP BY 1"
    );
    let mut stmt = ctx.index.connection().prepare_cached(&sql)?;
    let rows: Vec<(String, i64)> = stmt
        .query_map(
            rusqlite::params![snap.scan.id, NodeFlags::DELETED, cutoff],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<std::result::Result<_, _>>()?;
    let claimed: u64 = rows.iter().map(|(_, b)| *b as u64).sum();
    let mut out: Vec<BucketSlice> = rows
        .into_iter()
        .map(|(id, bytes)| slice(&ctx.rules, id, bytes as u64))
        .collect();
    if !t.stale_only {
        let rest = snap.root.size(t.size).saturating_sub(claimed);
        match out.iter_mut().find(|b| b.id == "other") {
            Some(o) => o.bytes += rest,
            None => out.push(slice(&ctx.rules, "other".into(), rest)),
        }
    }
    out.retain(|b| b.bytes > 0);
    out.sort_by_key(|b| std::cmp::Reverse(b.bytes));
    Ok(out)
}

fn slice(rules: &RuleSet, id: String, bytes: u64) -> BucketSlice {
    let def = rules.bucket(&id);
    BucketSlice {
        label: def.map_or_else(|| id.clone(), |b| b.label.clone()),
        colour: def.map_or_else(|| "#717171".into(), |b| b.colour.clone()),
        id,
        bytes,
    }
}

/// Enough rows to fill a tall terminal; the list scrolls past what fits.
/// Bytes per safety class. Everything no rule claims counts as protected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SafetySplit {
    pub regenerable: u64,
    pub cache: u64,
    pub review: u64,
    pub protected: u64,
    /// The part of regenerable plus cache that is idle past the stale threshold.
    pub stale_safe: u64,
}

impl SafetySplit {
    pub fn safe(&self) -> u64 {
        self.regenerable + self.cache
    }
}

/// Uses the safety each claim recorded at scan time (which includes
/// promotions such as `dist/` with a build script), falling back to the rule's class.
pub fn safety_split(ctx: &Ctx, snap: &Snapshot, t: Toggles) -> Result<SafetySplit> {
    let col = match t.size {
        SizeMode::Disk => "excl_disk",
        SizeMode::Apparent => "excl_apparent",
    };
    let cutoff = spacehog_core::index::now() - ctx.config.stale_days * 86_400;
    let sql = format!(
        "SELECT rule_id, safety, SUM({col}),
                SUM(CASE WHEN stale_ref IS NOT NULL AND stale_ref <= ?3 THEN {col} ELSE 0 END)
         FROM nodes WHERE scan_id = ?1 AND claim = 1 AND (flags & ?2) = 0
         GROUP BY rule_id, safety"
    );
    let mut stmt = ctx.index.connection().prepare_cached(&sql)?;
    let rows: Vec<(Option<String>, Option<String>, i64, i64)> = stmt
        .query_map(
            rusqlite::params![snap.scan.id, NodeFlags::DELETED, cutoff],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?
        .collect::<std::result::Result<_, _>>()?;
    let mut out = SafetySplit::default();
    let mut claimed = 0u64;
    for (rule, safety, bytes, stale) in rows {
        let (bytes, stale) = (bytes as u64, stale as u64);
        claimed += bytes;
        let class = safety
            .as_deref()
            .and_then(Safety::parse)
            .or_else(|| {
                rule.as_deref()
                    .and_then(|r| ctx.rules.get(r))
                    .map(|r| r.safety())
            })
            .unwrap_or(Safety::Protected);
        match class {
            Safety::Regenerable => out.regenerable += bytes,
            Safety::Cache => out.cache += bytes,
            Safety::Review => out.review += bytes,
            Safety::Protected => out.protected += bytes,
        }
        if matches!(class, Safety::Regenerable | Safety::Cache) {
            out.stale_safe += stale;
        }
    }
    out.protected += snap.root.size(t.size).saturating_sub(claimed);
    Ok(out)
}

pub const TOP_ROWS: usize = 120;

/// The largest categorised items under the session's toggles and filter.
pub fn top(
    ctx: &Ctx,
    snap: &Snapshot,
    t: Toggles,
    bucket: Option<&str>,
    filter: &str,
) -> Result<Vec<Item>> {
    if bucket == Some("other") {
        return Ok(Vec::new());
    }
    let q = TopQuery {
        limit: TOP_ROWS * 3,
        category: bucket.map(str::to_string),
        stale_days: t.stale_only.then_some(ctx.config.stale_days),
        ..Default::default()
    };
    let root = Path::new(&snap.root.path);
    let needle = filter.to_lowercase();
    let mut items: Vec<Item> = report::top(&ctx.index, &ctx.rules, snap.scan.id, &q)?
        .into_iter()
        .filter(|i| item_size(i, t.size) > 0)
        .filter(|i| t.hidden || !is_hidden_under(root, Path::new(&i.path)))
        .filter(|i| needle.is_empty() || i.path.to_lowercase().contains(&needle))
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(item_size(i, t.size)));
    items.truncate(TOP_ROWS);
    Ok(items)
}

pub fn item_size(i: &Item, mode: SizeMode) -> u64 {
    match mode {
        SizeMode::Disk => i.size,
        SizeMode::Apparent => i.apparent_size,
    }
}

fn is_hidden_under(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// Children of a node, largest first by `mode`.
pub fn children(ctx: &Ctx, id: NodeId, mode: SizeMode) -> Result<Vec<NodeInfo>> {
    let mut kids = ctx.index.children(id, 5_000)?;
    kids.retain(|n| !n.flags.has(NodeFlags::DELETED));
    kids.sort_by_key(|n| std::cmp::Reverse(n.size(mode)));
    Ok(kids)
}

/// Whether an Explorer row survives the toggles and the name filter. Open
/// folders always survive so a match deep in the tree stays reachable.
pub fn row_visible(n: &NodeInfo, t: Toggles, filter: &str, stale_days: i64, open: bool) -> bool {
    if !t.hidden && n.name.starts_with('.') {
        return false;
    }
    if open {
        return true;
    }
    if t.stale_only {
        let stale = n.stale_days.is_some_and(|d| d >= stale_days);
        let browsable = n.kind == NodeKind::Dir && !n.claim;
        if !stale && !browsable {
            return false;
        }
    }
    filter.is_empty() || n.name.to_lowercase().contains(&filter.to_lowercase())
}

/// Ancestors of `id` from the scan root down to `id` itself.
pub fn lineage(ctx: &Ctx, id: NodeId) -> Result<Vec<NodeId>> {
    let mut chain = vec![id];
    let mut cur = ctx.index.node(id)?.and_then(|n| n.parent_id);
    while let Some(p) = cur {
        chain.push(p);
        cur = ctx.index.node(p)?.and_then(|n| n.parent_id);
    }
    chain.reverse();
    Ok(chain)
}

/// Relative time like `3m ago` for the status strip.
pub fn ago(ts: i64) -> String {
    let secs = (spacehog_core::index::now() - ts).max(0);
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// Keeps both ends of a long path: `~/code/…/node_modules`.
pub fn truncate_middle(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let keep = width - 1;
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_middle_keeps_both_ends() {
        assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
        assert_eq!(truncate_middle("short", 10), "short");
        assert_eq!(truncate_middle("abcdefghij", 7).chars().count(), 7);
    }
}
