use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::Serialize;

use crate::model::{NodeFlags, NodeKind};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub root: PathBuf,
    pub one_file_system: bool,
    pub excludes: Vec<PathBuf>,
}

impl ScanOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let excludes = default_excludes(&root);
        Self {
            root,
            one_file_system: true,
            excludes,
        }
    }
}

/// Paths that are never descended unless the scan root is inside them.
pub fn default_excludes(root: &Path) -> Vec<PathBuf> {
    [
        "/proc",
        "/sys",
        "/dev",
        "/Volumes",
        "/System/Volumes",
        "/.MobileBackups",
        "/.Spotlight-V100",
        "/.fseventsd",
        "/private/var/vm",
        "/Volumes/com.apple.TimeMachine.localsnapshots",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|p| !root.starts_with(p))
    .collect()
}

/// Live counters shared between the walker and whoever renders progress.
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    /// Rows written to the index so far, during `Phase::Writing`.
    pub written: AtomicU64,
    pub cancelled: AtomicBool,
    phase: AtomicU8,
    current: Mutex<String>,
    finished_top: Mutex<Vec<TopLevelDir>>,
}

/// A direct child of the scan root whose walk has finished, so a progress
/// screen can show where the bytes are before the whole scan is done.
#[derive(Debug, Clone, Serialize)]
pub struct TopLevelDir {
    pub name: String,
    pub disk: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Walking,
    Labelling,
    Writing,
    Done,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProgressSnapshot {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub current_path: String,
    pub written: u64,
    pub phase: Phase,
}

impl Progress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            current_path: self.current.lock().map(|s| s.clone()).unwrap_or_default(),
            written: self.written.load(Ordering::Relaxed),
            phase: self.phase(),
        }
    }

    /// Finished top-level directories, largest first.
    pub fn finished_top_level(&self) -> Vec<TopLevelDir> {
        let mut v = self
            .finished_top
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default();
        v.sort_by_key(|d| std::cmp::Reverse(d.disk));
        v
    }

    fn record_top_level(&self, dir: TopLevelDir) {
        if let Ok(mut v) = self.finished_top.lock() {
            v.push(dir);
        }
    }

    pub fn phase(&self) -> Phase {
        match self.phase.load(Ordering::Relaxed) {
            0 => Phase::Walking,
            1 => Phase::Labelling,
            2 => Phase::Writing,
            _ => Phase::Done,
        }
    }

    pub fn set_phase(&self, phase: Phase) {
        let v = match phase {
            Phase::Walking => 0,
            Phase::Labelling => 1,
            Phase::Writing => 2,
            Phase::Done => 3,
        };
        self.phase.store(v, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn note_path(&self, path: &Path) {
        if let Ok(mut cur) = self.current.try_lock() {
            cur.clear();
            cur.push_str(&path.to_string_lossy());
        }
    }
}

/// Rolling files-per-second rate sampled at a fixed cadence.
#[derive(Debug, Clone)]
pub struct RateMeter {
    last: Instant,
    last_files: u64,
    pub samples: Vec<u64>,
    capacity: usize,
}

impl RateMeter {
    pub fn new(capacity: usize) -> Self {
        Self {
            last: Instant::now(),
            last_files: 0,
            samples: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn sample(&mut self, files: u64) -> u64 {
        let elapsed = self.last.elapsed();
        if elapsed < Duration::from_millis(40) {
            return self.samples.last().copied().unwrap_or(0);
        }
        let rate = ((files.saturating_sub(self.last_files)) as f64 / elapsed.as_secs_f64()) as u64;
        self.last = Instant::now();
        self.last_files = files;
        if self.samples.len() == self.capacity {
            self.samples.remove(0);
        }
        self.samples.push(rate);
        rate
    }
}

#[derive(Debug)]
struct RawNode {
    name: Box<str>,
    kind: NodeKind,
    apparent: u64,
    disk: u64,
    mtime: i64,
    atime: i64,
    ino: u64,
    dev: u64,
    nlink: u64,
    uid: u32,
    flags: NodeFlags,
    errno: i32,
    children: Vec<RawNode>,
}

/// A scanned tree flattened in pre-order. Every subtree is the contiguous
/// range `[i, last_desc]`, which the index relies on for descendant queries.
#[derive(Debug, Clone)]
pub struct FlatNode {
    pub parent: Option<u32>,
    pub last_desc: u32,
    pub depth: u16,
    pub name: Box<str>,
    pub kind: NodeKind,
    pub apparent: u64,
    pub disk: u64,
    pub file_count: u64,
    pub child_count: u32,
    pub mtime: i64,
    pub max_mtime: i64,
    pub atime: i64,
    pub ino: u64,
    pub dev: u64,
    pub nlink: u64,
    pub uid: u32,
    pub flags: NodeFlags,
    pub errno: i32,
}

#[derive(Debug, Clone)]
pub struct ScanTree {
    pub root: PathBuf,
    pub nodes: Vec<FlatNode>,
}

impl ScanTree {
    pub fn totals(&self) -> (u64, u64, u64, u64) {
        let root = &self.nodes[0];
        let dirs = self
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Dir)
            .count() as u64;
        (root.file_count, dirs, root.disk, root.apparent)
    }

    pub fn path_of(&self, idx: usize) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = Some(idx as u32);
        while let Some(i) = cur {
            let n = &self.nodes[i as usize];
            parts.push(n.name.as_ref());
            cur = n.parent;
        }
        let mut path = PathBuf::new();
        for p in parts.into_iter().rev() {
            path.push(p);
        }
        path
    }
}

/// What the walker needs to know about one entry, whichever API listed it.
#[derive(Debug, Clone, Copy)]
struct Stat {
    kind: NodeKind,
    apparent: u64,
    alloc: u64,
    mtime: i64,
    atime: i64,
    ino: u64,
    dev: u64,
    nlink: u64,
    uid: u32,
    cloud_only: bool,
}

/// Set on iCloud placeholders whose data lives only in the cloud.
const SF_DATALESS: u32 = 0x4000_0000;

impl Stat {
    fn of(meta: &Metadata) -> Self {
        let ft = meta.file_type();
        let kind = if ft.is_symlink() {
            NodeKind::Symlink
        } else if ft.is_dir() {
            NodeKind::Dir
        } else if ft.is_file() {
            NodeKind::File
        } else {
            NodeKind::Other
        };
        Stat {
            kind,
            apparent: meta.len(),
            alloc: meta.blocks() * 512,
            mtime: meta.mtime(),
            atime: meta.atime(),
            ino: meta.ino(),
            dev: meta.dev(),
            nlink: meta.nlink(),
            uid: meta.uid(),
            cloud_only: st_flags(meta) & SF_DATALESS != 0,
        }
    }
}

#[cfg(target_os = "macos")]
fn st_flags(meta: &Metadata) -> u32 {
    use std::os::macos::fs::MetadataExt as _;
    meta.st_flags()
}

#[cfg(not(target_os = "macos"))]
fn st_flags(_meta: &Metadata) -> u32 {
    0
}

type Listing = Vec<(Box<str>, std::result::Result<Stat, i32>)>;

/// Lists a directory with every entry's attributes. macOS reads them in bulk;
/// a filesystem without bulk support falls back to readdir plus lstat.
fn list(path: &Path) -> std::io::Result<Listing> {
    #[cfg(target_os = "macos")]
    match crate::bulk::read_dir(path) {
        Ok(entries) => {
            return Ok(entries
                .into_iter()
                .map(|e| {
                    let stat = if e.errno != 0 {
                        Err(e.errno)
                    } else {
                        Ok(Stat {
                            kind: e.kind,
                            apparent: e.apparent,
                            alloc: e.alloc,
                            mtime: e.mtime,
                            atime: e.atime,
                            ino: e.ino,
                            dev: e.dev,
                            nlink: e.nlink,
                            uid: e.uid,
                            cloud_only: e.st_flags & SF_DATALESS != 0,
                        })
                    };
                    (e.name, stat)
                })
                .collect());
        }
        Err(e)
            if e.raw_os_error() == Some(libc::ENOTSUP)
                || e.raw_os_error() == Some(libc::EINVAL) => {}
        Err(e) => return Err(e),
    }
    Ok(fs::read_dir(path)?
        .flatten()
        .map(|entry| {
            let name: Box<str> = entry.file_name().to_string_lossy().into();
            let stat = entry
                .metadata()
                .map(|m| Stat::of(&m))
                .map_err(|e| e.raw_os_error().unwrap_or(-1));
            (name, stat)
        })
        .collect())
}

struct Walker<'a> {
    opts: &'a ScanOptions,
    root_dev: u64,
    progress: &'a Progress,
    seen_inodes: Mutex<HashSet<(u64, u64)>>,
}

pub fn scan(opts: &ScanOptions, progress: &Progress) -> std::io::Result<ScanTree> {
    let root = fs::canonicalize(&opts.root)?;
    let stat = Stat::of(&fs::symlink_metadata(&root)?);
    let walker = Walker {
        opts,
        root_dev: stat.dev,
        progress,
        seen_inodes: Mutex::new(HashSet::new()),
    };
    let raw = walker.visit(&root, root.to_string_lossy().into(), stat, true);
    let mut nodes = Vec::with_capacity(progress.files.load(Ordering::Relaxed) as usize + 1024);
    flatten(raw, None, 0, &mut nodes);
    Ok(ScanTree { root, nodes })
}

impl Walker<'_> {
    fn visit(&self, path: &Path, name: Box<str>, stat: Stat, is_root: bool) -> RawNode {
        let mut node = self.leaf(name, &stat);
        if node.kind != NodeKind::Dir {
            return node;
        }
        self.progress.dirs.fetch_add(1, Ordering::Relaxed);
        if !is_root {
            if self.opts.excludes.iter().any(|e| e == path) {
                node.flags.set(NodeFlags::EXCLUDED);
                return node;
            }
            if self.opts.one_file_system && stat.dev != self.root_dev {
                node.kind = NodeKind::Mount;
                node.flags.set(NodeFlags::MOUNT);
                node.apparent = 0;
                node.disk = 0;
                return node;
            }
        }
        if self.progress.cancelled.load(Ordering::Relaxed) {
            return node;
        }
        let entries = match list(path) {
            Ok(entries) => entries,
            Err(e) => {
                node.flags.set(NodeFlags::ERROR);
                node.errno = e.raw_os_error().unwrap_or(-1);
                self.progress.errors.fetch_add(1, Ordering::Relaxed);
                return node;
            }
        };
        let mut subdirs = Vec::new();
        for (child_name, stat) in entries {
            let stat = match stat {
                Ok(s) => s,
                Err(errno) => {
                    self.progress.errors.fetch_add(1, Ordering::Relaxed);
                    node.children.push(RawNode::errored(child_name, errno));
                    continue;
                }
            };
            if stat.kind == NodeKind::Dir {
                subdirs.push((child_name, stat));
            } else {
                let files = self.progress.files.fetch_add(1, Ordering::Relaxed);
                if files.is_multiple_of(512) {
                    self.progress.note_path(&path.join(&*child_name));
                }
                node.children.push(self.leaf(child_name, &stat));
            }
        }
        let dirs: Vec<RawNode> = subdirs
            .into_par_iter()
            .map(|(n, s)| {
                let child = self.visit(&path.join(&*n), n, s, false);
                if is_root {
                    let (disk, files) = child.totals();
                    self.progress.record_top_level(TopLevelDir {
                        name: child.name.to_string(),
                        disk,
                        files,
                    });
                }
                child
            })
            .collect();
        node.children.extend(dirs);
        node
    }

    fn leaf(&self, name: Box<str>, stat: &Stat) -> RawNode {
        let mut flags = NodeFlags::default();
        let mut disk = stat.alloc;
        if stat.cloud_only {
            flags.set(NodeFlags::CLOUD_ONLY);
            disk = 0;
        }
        if stat.kind == NodeKind::File && stat.nlink > 1 {
            let first = self
                .seen_inodes
                .lock()
                .map(|mut s| s.insert((stat.dev, stat.ino)))
                .unwrap_or(true);
            if !first {
                disk = 0;
            }
        }
        self.progress.bytes.fetch_add(disk, Ordering::Relaxed);
        RawNode {
            name,
            kind: stat.kind,
            apparent: stat.apparent,
            disk,
            mtime: stat.mtime,
            atime: stat.atime,
            ino: stat.ino,
            dev: stat.dev,
            nlink: stat.nlink,
            uid: stat.uid,
            flags,
            errno: 0,
            children: Vec::new(),
        }
    }
}

impl RawNode {
    fn totals(&self) -> (u64, u64) {
        let own_file = u64::from(self.kind != NodeKind::Dir && self.kind != NodeKind::Mount);
        self.children
            .iter()
            .fold((self.disk, own_file), |(d, f), c| {
                let (cd, cf) = c.totals();
                (d + cd, f + cf)
            })
    }

    fn errored(name: Box<str>, errno: i32) -> Self {
        RawNode {
            name,
            kind: NodeKind::Other,
            apparent: 0,
            disk: 0,
            mtime: 0,
            atime: 0,
            ino: 0,
            dev: 0,
            nlink: 0,
            uid: 0,
            flags: NodeFlags(NodeFlags::ERROR),
            errno,
            children: Vec::new(),
        }
    }
}

fn flatten(raw: RawNode, parent: Option<u32>, depth: u16, out: &mut Vec<FlatNode>) -> usize {
    let idx = out.len();
    let is_file = matches!(
        raw.kind,
        NodeKind::File | NodeKind::Symlink | NodeKind::Other
    );
    out.push(FlatNode {
        parent,
        last_desc: idx as u32,
        depth,
        name: raw.name,
        kind: raw.kind,
        apparent: raw.apparent,
        disk: raw.disk,
        file_count: u64::from(is_file),
        child_count: raw.children.len() as u32,
        mtime: raw.mtime,
        max_mtime: raw.mtime,
        atime: raw.atime,
        ino: raw.ino,
        dev: raw.dev,
        nlink: raw.nlink,
        uid: raw.uid,
        flags: raw.flags,
        errno: raw.errno,
    });
    let (mut apparent, mut disk, mut files, mut max_mtime) = (0u64, 0u64, 0u64, raw.mtime);
    for child in raw.children {
        let c = flatten(child, Some(idx as u32), depth.saturating_add(1), out);
        let c = &out[c];
        apparent += c.apparent;
        disk += c.disk;
        files += c.file_count;
        max_mtime = max_mtime.max(c.max_mtime);
    }
    let last = out.len() as u32 - 1;
    let n = &mut out[idx];
    n.last_desc = last;
    n.apparent += apparent;
    n.disk += disk;
    n.file_count += files;
    n.max_mtime = max_mtime;
    idx
}
