use std::path::Path;

use rusqlite::ToSql;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::index::{Index, now};
use crate::model::{NodeFlags, NodeInfo, NodeKind, ScanId};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindFilter {
    Dir,
    File,
    #[default]
    Any,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Size,
    Mtime,
}

/// Index-level filter. Higher layers resolve bucket, rule, and safety
/// filters to a concrete rule id set before calling.
#[derive(Debug, Clone, Default)]
pub struct NodeFilter {
    pub claims_only: bool,
    pub rule_ids: Option<Vec<String>>,
    pub buckets: Option<Vec<String>>,
    pub min_size: Option<u64>,
    pub stale_days: Option<i64>,
    pub under: Option<std::path::PathBuf>,
    pub kind: KindFilter,
    pub name_glob: Option<String>,
    pub max_depth: Option<u32>,
    pub sort: Sort,
    pub limit: usize,
    pub include_deleted: bool,
    pub safety: Option<Vec<crate::rules::Safety>>,
}

impl Index {
    pub fn require_scan(&self, scan: Option<ScanId>) -> Result<ScanId> {
        match scan {
            Some(id) => Ok(id),
            None => self.latest_scan(None)?.map(|s| s.id).ok_or(Error::NoScan),
        }
    }

    pub fn select(&self, scan: ScanId, f: &NodeFilter) -> Result<Vec<NodeInfo>> {
        let mut clause = String::from("WHERE scan_id = ?");
        let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(scan)];
        if f.claims_only {
            clause.push_str(" AND claim = 1");
        }
        if !f.include_deleted {
            clause.push_str(&format!(" AND (flags & {}) = 0", NodeFlags::DELETED));
        }
        if let Some(ids) = &f.rule_ids {
            clause.push_str(&format!(" AND rule_id IN ({})", placeholders(ids.len())));
            args.extend(ids.iter().map(|s| Box::new(s.clone()) as Box<dyn ToSql>));
        }
        if let Some(b) = &f.buckets {
            clause.push_str(&format!(" AND bucket IN ({})", placeholders(b.len())));
            args.extend(b.iter().map(|s| Box::new(s.clone()) as Box<dyn ToSql>));
        }
        let size_col = if f.claims_only { "excl_disk" } else { "disk" };
        if let Some(min) = f.min_size {
            clause.push_str(&format!(" AND {size_col} >= ?"));
            args.push(Box::new(min as i64));
        }
        if let Some(classes) = &f.safety {
            clause.push_str(&format!(" AND safety IN ({})", placeholders(classes.len())));
            args.extend(
                classes
                    .iter()
                    .map(|s| Box::new(s.as_str()) as Box<dyn ToSql>),
            );
        }
        if let Some(days) = f.stale_days {
            clause.push_str(" AND stale_ref IS NOT NULL AND stale_ref <= ?");
            args.push(Box::new(now() - days * 86_400));
        }
        match f.kind {
            KindFilter::Dir => clause.push_str(&format!(" AND kind = {}", NodeKind::Dir.as_i64())),
            KindFilter::File => {
                clause.push_str(&format!(" AND kind != {}", NodeKind::Dir.as_i64()))
            }
            KindFilter::Any => {}
        }
        if let Some(under) = &f.under {
            let node = self
                .find(scan, under)?
                .ok_or_else(|| Error::NotIndexed(under.clone()))?;
            let last: i64 = self.conn.query_row(
                "SELECT last_desc FROM nodes WHERE id = ?1",
                [node.id],
                |r| r.get(0),
            )?;
            clause.push_str(" AND id > ? AND id <= ?");
            args.push(Box::new(node.id));
            args.push(Box::new(last));
            if let Some(depth) = f.max_depth {
                clause.push_str(" AND depth <= (SELECT depth FROM nodes WHERE id = ?) + ?");
                args.push(Box::new(node.id));
                args.push(Box::new(depth as i64));
            }
        } else if let Some(depth) = f.max_depth {
            clause.push_str(" AND depth <= ?");
            args.push(Box::new(depth as i64));
        }
        if let Some(glob) = &f.name_glob {
            clause.push_str(" AND name GLOB ?");
            args.push(Box::new(glob.clone()));
        }
        clause.push_str(&match f.sort {
            Sort::Size => format!(" ORDER BY {size_col} DESC"),
            Sort::Mtime => " ORDER BY max_mtime ASC".to_string(),
        });
        clause.push_str(" LIMIT ?");
        args.push(Box::new(if f.limit == 0 { 20 } else { f.limit } as i64));
        let refs: Vec<&dyn ToSql> = args.iter().map(|b| b.as_ref()).collect();
        self.nodes_where(&clause, &refs)
    }

    pub fn find_in_latest(&self, path: &Path) -> Result<Option<NodeInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM scans WHERE status = 'done' ORDER BY finished_at DESC, id DESC",
        )?;
        let ids: Vec<ScanId> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for id in ids {
            if let Some(n) = self.find(id, path)? {
                return Ok(Some(n));
            }
        }
        Ok(None)
    }
}

fn placeholders(n: usize) -> String {
    vec!["?"; n.max(1)].join(", ")
}
