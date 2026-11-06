use std::collections::HashMap;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::human;
use crate::index::{Index, now};
use crate::model::{NodeInfo, NodeKind, ScanId, ScanRecord};
use crate::report;
use crate::rules::RuleSet;

/// Changes smaller than this are noise: logs ticking over, caches churning.
const MIN_DELTA: i64 = 1_000_000;
/// A directory whose change is at least this share one child's is reported as that child.
const DOMINANT_SHARE: f64 = 0.8;

#[derive(Debug, Clone, Copy)]
pub enum Since {
    Previous,
    Scan(ScanId),
    /// The newest scan finished at least this many seconds ago.
    Ago(i64),
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketDelta {
    pub id: String,
    pub label: String,
    pub colour: String,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
    pub delta_human: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Grown,
    Shrunk,
    New,
    Gone,
}

#[derive(Debug, Clone, Serialize)]
pub struct PathDelta {
    pub path: String,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
    pub delta_human: String,
    pub change: ChangeKind,
    pub bucket: Option<String>,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Changes {
    pub root: String,
    pub from: ScanRecord,
    pub to: ScanRecord,
    pub total_delta: i64,
    pub total_delta_human: String,
    pub buckets: Vec<BucketDelta>,
    pub grown: Vec<PathDelta>,
    pub shrunk: Vec<PathDelta>,
}

pub fn signed(delta: i64) -> String {
    let sign = if delta < 0 { "-" } else { "+" };
    format!("{sign}{}", human::bytes(delta.unsigned_abs()))
}

fn pick_scans(index: &Index, root: Option<&str>, since: Since) -> Result<(ScanRecord, ScanRecord)> {
    let to = match root {
        Some(r) => index.latest_scan(Some(std::path::Path::new(r)))?,
        None => index.latest_scan(None)?,
    }
    .ok_or(Error::NoScan)?;
    let history = index.scans_for_root(&to.root)?;
    let from = match since {
        Since::Scan(id) => history.into_iter().find(|s| s.id == id),
        Since::Previous => history.into_iter().find(|s| s.id != to.id),
        Since::Ago(secs) => {
            let cutoff = now() - secs;
            let older: Vec<_> = history.into_iter().filter(|s| s.id != to.id).collect();
            let at_cutoff = older
                .iter()
                .find(|s| s.finished_at.unwrap_or(0) <= cutoff)
                .cloned();
            at_cutoff.or_else(|| older.last().cloned())
        }
    };
    let from = from.ok_or_else(|| {
        Error::Invalid(format!(
            "only one scan of {} is kept; rescan later to see changes",
            to.root
        ))
    })?;
    Ok((from, to))
}

pub fn changes(
    index: &Index,
    rules: &RuleSet,
    root: Option<&str>,
    since: Since,
    limit: usize,
) -> Result<Changes> {
    let (from, to) = pick_scans(index, root, since)?;
    let before = report::summary(index, rules, from.id, 90)?;
    let after = report::summary(index, rules, to.id, 90)?;
    let mut buckets: HashMap<String, (u64, u64)> = HashMap::new();
    for b in &before.buckets {
        buckets.entry(b.id.clone()).or_default().0 = b.bytes;
    }
    for b in &after.buckets {
        buckets.entry(b.id.clone()).or_default().1 = b.bytes;
    }
    let mut bucket_deltas: Vec<BucketDelta> = buckets
        .into_iter()
        .map(|(id, (b, a))| {
            let def = rules.bucket(&id);
            let delta = a as i64 - b as i64;
            BucketDelta {
                label: def.map_or_else(|| id.clone(), |d| d.label.clone()),
                colour: def.map_or_else(|| "#717171".into(), |d| d.colour.clone()),
                id,
                before: b,
                after: a,
                delta,
                delta_human: signed(delta),
            }
        })
        .filter(|d| d.delta != 0)
        .collect();
    bucket_deltas.sort_by_key(|d| std::cmp::Reverse(d.delta.abs()));

    let old_root = index.root_node(from.id)?.ok_or(Error::NoScan)?;
    let new_root = index.root_node(to.id)?.ok_or(Error::NoScan)?;
    let mut found = Vec::new();
    let mut walk = Walk {
        index,
        old_compacted: index.is_compacted(from.id)?,
    };
    walk.attribute(Some(&old_root), Some(&new_root), &mut found)?;
    let (mut grown, mut shrunk): (Vec<_>, Vec<_>) = found.into_iter().partition(|d| d.delta > 0);
    grown.sort_by_key(|d| std::cmp::Reverse(d.delta));
    shrunk.sort_by_key(|d| d.delta);
    grown.truncate(limit);
    shrunk.truncate(limit);
    let total = new_root.disk_size as i64 - old_root.disk_size as i64;
    Ok(Changes {
        root: to.root.clone(),
        from,
        to,
        total_delta: total,
        total_delta_human: signed(total),
        buckets: bucket_deltas,
        grown,
        shrunk,
    })
}

fn delta_of(old: Option<&NodeInfo>, new: Option<&NodeInfo>) -> i64 {
    new.map_or(0, |n| n.disk_size as i64) - old.map_or(0, |n| n.disk_size as i64)
}

struct Walk<'a> {
    index: &'a Index,
    /// The older scan dropped small unclaimed files, so the newer side must
    /// ignore them too or every one would look new.
    old_compacted: bool,
}

impl Walk<'_> {
    fn children_by_name(
        &self,
        node: Option<&NodeInfo>,
        is_new: bool,
    ) -> Result<HashMap<String, NodeInfo>> {
        let Some(n) = node.filter(|n| n.kind == NodeKind::Dir) else {
            return Ok(HashMap::new());
        };
        let min = crate::index::COMPACT_MIN_FILE as u64;
        let comparable = |c: &NodeInfo| {
            !(is_new && self.old_compacted)
                || c.kind == NodeKind::Dir
                || c.claim
                || c.disk_size >= min
        };
        Ok(self
            .index
            .children(n.id, usize::MAX >> 1)?
            .into_iter()
            .filter(comparable)
            .map(|c| (c.name.clone(), c))
            .collect())
    }

    /// Reports where a change lives: descends while one child dominates the
    /// change, and reports a node when its change is spread across children.
    fn attribute(
        &mut self,
        old: Option<&NodeInfo>,
        new: Option<&NodeInfo>,
        out: &mut Vec<PathDelta>,
    ) -> Result<()> {
        let delta = delta_of(old, new);
        if delta.abs() < MIN_DELTA {
            return Ok(());
        }
        let old_kids = self.children_by_name(old, false)?;
        let new_kids = self.children_by_name(new, true)?;
        let mut kids: Vec<(Option<&NodeInfo>, Option<&NodeInfo>, i64)> = old_kids
            .iter()
            .map(|(name, o)| (Some(o), new_kids.get(name)))
            .chain(
                new_kids
                    .iter()
                    .filter(|(n, _)| !old_kids.contains_key(*n))
                    .map(|(_, n)| (None, Some(n))),
            )
            .map(|(o, n)| (o, n, delta_of(o, n)))
            .filter(|(_, _, d)| d.abs() >= MIN_DELTA)
            .collect();
        kids.sort_by_key(|(_, _, d)| std::cmp::Reverse(d.abs()));

        let dominated = kids.first().is_some_and(|(_, _, d)| {
            d.signum() == delta.signum() && (*d as f64 / delta as f64) >= DOMINANT_SHARE
        });
        let both_present = old.is_some() && new.is_some();
        if !dominated || !both_present {
            let node = new.or(old).expect("one side exists");
            out.push(PathDelta {
                path: node.path.clone(),
                before: old.map_or(0, |n| n.disk_size),
                after: new.map_or(0, |n| n.disk_size),
                delta,
                delta_human: signed(delta),
                change: match (old.is_some(), new.is_some(), delta > 0) {
                    (false, _, _) => ChangeKind::New,
                    (_, false, _) => ChangeKind::Gone,
                    (_, _, true) => ChangeKind::Grown,
                    _ => ChangeKind::Shrunk,
                },
                bucket: node.bucket.clone(),
                kind: node.kind,
            });
        }
        if both_present {
            for (o, n, _) in kids {
                self.attribute(o, n, out)?;
            }
        }
        Ok(())
    }
}
