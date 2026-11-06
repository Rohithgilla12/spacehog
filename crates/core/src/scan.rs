use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    pub done: AtomicBool,
    pub cancelled: AtomicBool,
    current: Mutex<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProgressSnapshot {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub current_path: String,
    pub done: bool,
}

impl Progress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            current_path: self.current.lock().map(|s| s.clone()).unwrap_or_default(),
            done: self.done.load(Ordering::Relaxed),
        }
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

struct Walker<'a> {
    opts: &'a ScanOptions,
    root_dev: u64,
    progress: &'a Progress,
    seen_inodes: Mutex<HashSet<(u64, u64)>>,
}

pub fn scan(opts: &ScanOptions, progress: &Progress) -> std::io::Result<ScanTree> {
    let root = fs::canonicalize(&opts.root)?;
    let meta = fs::symlink_metadata(&root)?;
    let walker = Walker {
        opts,
        root_dev: meta.dev(),
        progress,
        seen_inodes: Mutex::new(HashSet::new()),
    };
    let raw = walker.visit(&root, root.to_string_lossy().into(), meta, true);
    progress.done.store(true, Ordering::Relaxed);
    let mut nodes = Vec::with_capacity(progress.files.load(Ordering::Relaxed) as usize + 1024);
    flatten(raw, None, 0, &mut nodes);
    Ok(ScanTree { root, nodes })
}

impl Walker<'_> {
    fn visit(&self, path: &Path, name: Box<str>, meta: Metadata, is_root: bool) -> RawNode {
        let mut node = self.leaf(name, &meta);
        if node.kind != NodeKind::Dir {
            return node;
        }
        self.progress.dirs.fetch_add(1, Ordering::Relaxed);
        if !is_root {
            if self.opts.excludes.iter().any(|e| e == path) {
                node.flags.set(NodeFlags::EXCLUDED);
                return node;
            }
            if self.opts.one_file_system && meta.dev() != self.root_dev {
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
        let entries = match fs::read_dir(path) {
            Ok(rd) => rd,
            Err(e) => {
                node.flags.set(NodeFlags::ERROR);
                node.errno = e.raw_os_error().unwrap_or(-1);
                self.progress.errors.fetch_add(1, Ordering::Relaxed);
                return node;
            }
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let child_path = entry.path();
            let child_name: Box<str> = entry.file_name().to_string_lossy().into();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    self.progress.errors.fetch_add(1, Ordering::Relaxed);
                    node.children.push(RawNode {
                        name: child_name,
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
                        errno: e.raw_os_error().unwrap_or(-1),
                        children: Vec::new(),
                    });
                    continue;
                }
            };
            if meta.is_dir() {
                subdirs.push((child_path, child_name, meta));
            } else {
                let leaf = self.leaf(child_name, &meta);
                let files = self.progress.files.fetch_add(1, Ordering::Relaxed);
                if files.is_multiple_of(512) {
                    self.progress.note_path(&child_path);
                }
                node.children.push(leaf);
            }
        }
        let dirs: Vec<RawNode> = subdirs
            .into_par_iter()
            .map(|(p, n, m)| self.visit(&p, n, m, false))
            .collect();
        node.children.extend(dirs);
        node
    }

    fn leaf(&self, name: Box<str>, meta: &Metadata) -> RawNode {
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
        let mut flags = NodeFlags::default();
        let mut disk = meta.blocks() * 512;
        if cloud_only(meta) {
            flags.set(NodeFlags::CLOUD_ONLY);
            disk = 0;
        }
        if kind == NodeKind::File && meta.nlink() > 1 {
            let key = (meta.dev(), meta.ino());
            let first = self
                .seen_inodes
                .lock()
                .map(|mut s| s.insert(key))
                .unwrap_or(true);
            if !first {
                disk = 0;
            }
        }
        self.progress.bytes.fetch_add(disk, Ordering::Relaxed);
        RawNode {
            name,
            kind,
            apparent: meta.len(),
            disk,
            mtime: meta.mtime(),
            atime: meta.atime(),
            ino: meta.ino(),
            dev: meta.dev(),
            nlink: meta.nlink(),
            uid: meta.uid(),
            flags,
            errno: 0,
            children: Vec::new(),
        }
    }
}

#[cfg(target_os = "macos")]
fn cloud_only(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    const SF_DATALESS: u32 = 0x4000_0000;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn cloud_only(_meta: &Metadata) -> bool {
    false
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
