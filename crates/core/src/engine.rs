use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::index::Index;
use crate::lock::ScanLock;
use crate::model::{Labels, ScanId, ScanRecord};
use crate::scan::{self, Phase, Progress, ScanOptions, ScanTree};

/// Labels a freshly scanned tree before it is written. The categoriser
/// implements this; a scan without one is written uncategorised.
pub trait Labeler: Send + Sync {
    fn label(&self, tree: &ScanTree) -> Labels;
}

pub struct ScanRequest {
    pub options: ScanOptions,
    pub db: PathBuf,
    pub lock_dir: PathBuf,
    pub keep: usize,
    pub labeler: Option<Arc<dyn Labeler>>,
}

impl ScanRequest {
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

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ScanTimings {
    pub walk_ms: u64,
    pub label_ms: u64,
    pub write_ms: u64,
    pub nodes: u64,
}

pub type ScanOutcome = (ScanRecord, ScanTimings);

/// A scan running on a background thread. The id is known up front so
/// callers can report it before the walk finishes.
pub struct ScanJob {
    pub scan_id: ScanId,
    pub root: PathBuf,
    pub progress: Arc<Progress>,
    handle: std::thread::JoinHandle<Result<ScanOutcome>>,
}

impl ScanJob {
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn join(self) -> Result<ScanOutcome> {
        self.handle
            .join()
            .unwrap_or_else(|_| Err(crate::Error::Invalid("scan thread panicked".into())))
    }
}

struct Started {
    index: Index,
    scan_id: ScanId,
    lock: ScanLock,
    options: ScanOptions,
}

/// Opens the index, records the scan, and takes the lock. A held lock fails
/// here, before any thread is spawned.
fn start(req: &ScanRequest) -> Result<Started> {
    let root = std::fs::canonicalize(&req.options.root)?;
    let index = Index::open(&req.db)?;
    let scan_id = index.begin_scan(&root)?;
    let lock = match ScanLock::acquire(&req.lock_dir, scan_id) {
        Ok(l) => l,
        Err(e) => {
            index.fail_scan(scan_id, &e.to_string())?;
            return Err(e);
        }
    };
    let mut options = req.options.clone();
    options.root = root;
    Ok(Started {
        index,
        scan_id,
        lock,
        options,
    })
}

/// Walks, labels, and writes. Progress is mirrored into the `scans` row
/// every 250 ms so other processes can read it.
fn body(req: &ScanRequest, started: Started, progress: Arc<Progress>) -> Result<ScanOutcome> {
    let Started {
        mut index,
        scan_id,
        lock,
        options,
    } = started;
    let _lock = lock;
    let mut timings = ScanTimings::default();
    let mut clock = Instant::now();
    let mut lap = |slot: &mut u64| {
        *slot = clock.elapsed().as_millis() as u64;
        clock = Instant::now();
    };
    let mirror = spawn_mirror(req.db.clone(), scan_id, progress.clone());
    let walked = scan::scan(&options, &progress);
    lap(&mut timings.walk_ms);
    let tree = match walked {
        Ok(t) => t,
        Err(e) => {
            progress.set_phase(Phase::Done);
            let _ = mirror.join();
            index.fail_scan(scan_id, &e.to_string())?;
            return Err(e.into());
        }
    };
    progress.set_phase(Phase::Labelling);
    let labels = req.labeler.as_ref().map(|l| l.label(&tree));
    lap(&mut timings.label_ms);
    timings.nodes = tree.nodes.len() as u64;
    progress.set_phase(Phase::Writing);
    let written = index.finish_scan(scan_id, &tree, labels.as_ref(), req.keep, Some(&progress));
    progress.set_phase(Phase::Done);
    let _ = mirror.join();
    if let Err(e) = written {
        index.fail_scan(scan_id, &e.to_string())?;
        return Err(e);
    }
    lap(&mut timings.write_ms);
    Ok((index.scan(scan_id)?.expect("scan row exists"), timings))
}

pub fn run_scan(req: &ScanRequest, progress: Arc<Progress>) -> Result<ScanRecord> {
    run_scan_timed(req, progress).map(|(r, _)| r)
}

pub fn run_scan_timed(req: &ScanRequest, progress: Arc<Progress>) -> Result<ScanOutcome> {
    let started = start(req)?;
    body(req, started, progress)
}

pub fn spawn_scan(req: ScanRequest) -> Result<ScanJob> {
    let started = start(&req)?;
    let scan_id = started.scan_id;
    let root = started.options.root.clone();
    let progress = Arc::new(Progress::default());
    let p = progress.clone();
    let handle = std::thread::Builder::new()
        .name(format!("scan-{scan_id}"))
        .spawn(move || body(&req, started, p))?;
    Ok(ScanJob {
        scan_id,
        root,
        progress,
        handle,
    })
}

fn spawn_mirror(
    db: PathBuf,
    scan_id: ScanId,
    progress: Arc<Progress>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Ok(index) = Index::open(&db) else { return };
        while progress.phase() != Phase::Done {
            let _ = index.update_progress(scan_id, &progress.snapshot());
            std::thread::sleep(Duration::from_millis(250));
        }
    })
}

pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Resolves the parent directory but not the final component, so a symlink
/// names the link itself rather than its target.
pub fn canonical_entry(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            canonical(parent).join(name)
        }
        _ => canonical(path),
    }
}
