use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::error::Result;
use crate::index::Index;
use crate::lock::ScanLock;
use crate::model::{Label, ScanId, ScanRecord};
use crate::scan::{self, Progress, ScanOptions, ScanTree};

/// Labels a freshly scanned tree before it is written. The categoriser
/// implements this; a scan without one is written uncategorised.
pub trait Labeler: Send + Sync {
    fn label(&self, tree: &ScanTree) -> Vec<Label>;
}

pub struct ScanRequest<'a> {
    pub options: ScanOptions,
    pub db: PathBuf,
    pub lock_dir: PathBuf,
    pub keep: usize,
    pub labeler: Option<&'a dyn Labeler>,
}

impl ScanRequest<'_> {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            options: ScanOptions::new(root),
            db: crate::paths::db_path(),
            lock_dir: crate::paths::data_dir(),
            keep: 5,
            labeler: None,
        }
    }
}

/// Takes the scan lock, walks, labels, and writes the tree. Progress is
/// mirrored into the `scans` row every 250 ms so other processes can read it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ScanTimings {
    pub walk_ms: u64,
    pub label_ms: u64,
    pub write_ms: u64,
    pub nodes: u64,
}

pub fn run_scan(req: &ScanRequest<'_>, progress: Arc<Progress>) -> Result<ScanRecord> {
    run_scan_timed(req, progress).map(|(r, _)| r)
}

pub fn run_scan_timed(
    req: &ScanRequest<'_>,
    progress: Arc<Progress>,
) -> Result<(ScanRecord, ScanTimings)> {
    let mut timings = ScanTimings::default();
    let mut clock = std::time::Instant::now();
    let mut lap = |slot: &mut u64| {
        *slot = clock.elapsed().as_millis() as u64;
        clock = std::time::Instant::now();
    };
    let root = std::fs::canonicalize(&req.options.root)?;
    let mut index = Index::open(&req.db)?;
    let scan_id = index.begin_scan(&root)?;
    let _lock = match ScanLock::acquire(&req.lock_dir, scan_id) {
        Ok(l) => l,
        Err(e) => {
            index.fail_scan(scan_id, &e.to_string())?;
            return Err(e);
        }
    };
    let mut options = req.options.clone();
    options.root = root;
    let mirror = spawn_mirror(req.db.clone(), scan_id, progress.clone());
    lap(&mut 0);
    let walked = scan::scan(&options, &progress);
    progress
        .done
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = mirror.join();
    lap(&mut timings.walk_ms);
    let tree = match walked {
        Ok(t) => t,
        Err(e) => {
            index.fail_scan(scan_id, &e.to_string())?;
            return Err(e.into());
        }
    };
    let labels = req.labeler.map(|l| l.label(&tree));
    lap(&mut timings.label_ms);
    timings.nodes = tree.nodes.len() as u64;
    index.finish_scan(scan_id, &tree, labels.as_deref(), req.keep)?;
    lap(&mut timings.write_ms);
    Ok((index.scan(scan_id)?.expect("scan row exists"), timings))
}

fn spawn_mirror(
    db: PathBuf,
    scan_id: ScanId,
    progress: Arc<Progress>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Ok(index) = Index::open(&db) else { return };
        while !progress.done.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = index.update_progress(scan_id, &progress.snapshot());
            std::thread::sleep(Duration::from_millis(250));
        }
    })
}

pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
