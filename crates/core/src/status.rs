use serde::Serialize;

use crate::error::Result;
use crate::human;
use crate::index::Index;
use crate::model::ScanRecord;

#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub total_human: String,
    pub used_human: String,
    pub free_human: String,
    pub used_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub volumes: Vec<Volume>,
    pub last_scan: Option<ScanRecord>,
    pub scan_in_progress: Option<ScanRecord>,
    pub pending_proposals: u64,
}

pub fn volumes() -> Vec<Volume> {
    let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Volume> = disks
        .iter()
        .filter(|d| d.total_space() > 0)
        .filter(|d| {
            let m = d.mount_point().to_string_lossy();
            !m.starts_with("/System/Volumes/") || m == "/System/Volumes/Data"
        })
        .filter(|d| seen.insert((d.total_space(), d.available_space())))
        .map(|d| {
            let total = d.total_space();
            let free = d.available_space();
            let used = total.saturating_sub(free);
            Volume {
                mount: d.mount_point().to_string_lossy().into_owned(),
                total,
                used,
                free,
                total_human: human::bytes(total),
                used_human: human::bytes(used),
                free_human: human::bytes(free),
                used_pct: (used as f64 / total as f64 * 1000.0).round() / 10.0,
            }
        })
        .collect();
    out.sort_by_key(|a| a.mount.len());
    out
}

/// The volume holding `path`: the longest mount point that prefixes it.
pub fn volume_for(path: &std::path::Path) -> Option<Volume> {
    volumes()
        .into_iter()
        .filter(|v| path.starts_with(&v.mount))
        .max_by_key(|v| v.mount.len())
}

pub fn status(index: &Index) -> Result<Status> {
    let pending: i64 = index.connection().query_row(
        "SELECT COUNT(*) FROM proposals WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?;
    Ok(Status {
        volumes: volumes(),
        last_scan: index.latest_scan(None)?,
        scan_in_progress: index.scan_in_progress()?,
        pending_proposals: pending as u64,
    })
}
