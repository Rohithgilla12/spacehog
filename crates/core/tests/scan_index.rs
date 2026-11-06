use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use spacehog_core::Index;
use spacehog_core::engine::{ScanRequest, run_scan};
use spacehog_core::model::{NodeFlags, NodeKind};
use spacehog_core::query::{KindFilter, NodeFilter};
use spacehog_core::scan::{Progress, ScanOptions, scan};

struct Fixture {
    dir: tempfile::TempDir,
    file_bytes: u64,
    files: u64,
}

impl Fixture {
    fn root(&self) -> PathBuf {
        fs::canonicalize(self.dir.path()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let locked = self.dir.path().join("locked");
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
    }
}

/// 10,000 regular files across 100 dirs, one hardlinked 1 MB file, a
/// symlink to a large dir, and an unreadable dir.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut file_bytes = 0;
    for d in 0..100 {
        let sub = root.join(format!("d{d:03}"));
        fs::create_dir(&sub).unwrap();
        for f in 0..100 {
            let len = (d * 100 + f) % 5000;
            fs::write(sub.join(format!("f{f:03}")), vec![b'x'; len]).unwrap();
            file_bytes += len as u64;
        }
    }
    let big = root.join("big");
    fs::create_dir(&big).unwrap();
    fs::write(big.join("blob"), vec![7u8; 1 << 20]).unwrap();
    fs::hard_link(big.join("blob"), root.join("blob-link")).unwrap();
    file_bytes += 2 << 20;
    std::os::unix::fs::symlink(&big, root.join("big-link")).unwrap();
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("secret"), b"hidden").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    Fixture {
        dir,
        file_bytes,
        files: 10_002,
    }
}

fn running_as_root() -> bool {
    unsafe { libc_geteuid() == 0 }
}

unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

#[test]
fn walk_counts_files_hardlinks_symlinks_and_errors() {
    let fx = fixture();
    let tree = scan(&ScanOptions::new(fx.root()), &Progress::default()).unwrap();
    let files: Vec<_> = tree
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .collect();
    assert_eq!(
        files.len() as u64,
        fx.files,
        "every regular file is recorded once per name"
    );
    let apparent: u64 = files.iter().map(|n| n.apparent).sum();
    assert_eq!(
        apparent, fx.file_bytes,
        "apparent size counts each hardlink name"
    );

    let blobs: Vec<_> = files.iter().filter(|n| n.apparent == 1 << 20).collect();
    assert_eq!(blobs.len(), 2);
    assert_eq!(
        blobs.iter().filter(|n| n.disk > 0).count(),
        1,
        "a hardlinked inode is allocated once"
    );

    let link = tree.nodes.iter().find(|n| &*n.name == "big-link").unwrap();
    assert_eq!(link.kind, NodeKind::Symlink);
    assert!(link.disk < 1 << 20, "symlinks are never followed");

    if !running_as_root() {
        let locked = tree.nodes.iter().find(|n| &*n.name == "locked").unwrap();
        assert!(locked.flags.has(NodeFlags::ERROR));
        assert_eq!(locked.errno, 13, "EACCES recorded on the node");
        assert_eq!(locked.child_count, 0);
    }

    let root = &tree.nodes[0];
    assert_eq!(
        root.file_count,
        tree.nodes
            .iter()
            .filter(|n| n.kind != NodeKind::Dir)
            .count() as u64
    );
    for (i, n) in tree.nodes.iter().enumerate() {
        let kids: u64 = tree.nodes[i + 1..=n.last_desc as usize]
            .iter()
            .filter(|c| c.parent == Some(i as u32))
            .map(|c| c.disk)
            .sum();
        assert!(n.disk >= kids, "a node's total covers its children");
    }
}

fn db_in(dir: &Path) -> PathBuf {
    dir.join("index.db")
}

fn scan_into(root: &Path, db: &Path, keep: usize) -> spacehog_core::model::ScanRecord {
    let mut req = ScanRequest::new(root);
    req.db = db.to_path_buf();
    req.lock_dir = db.parent().unwrap().to_path_buf();
    req.keep = keep;
    run_scan(&req, Arc::new(Progress::default())).unwrap()
}

#[test]
fn index_round_trips_paths_and_totals() {
    let fx = fixture();
    let state = tempfile::tempdir().unwrap();
    let db = db_in(state.path());
    let rec = scan_into(&fx.root(), &db, 5);
    assert_eq!(
        rec.files,
        fx.files + 1,
        "regular files plus the symlink, nothing behind EACCES"
    );

    let index = Index::open(&db).unwrap();
    let blob = fx.root().join("big/blob");
    let node = index.find(rec.id, &blob).unwrap().expect("blob indexed");
    assert_eq!(node.path, blob.to_string_lossy());
    assert_eq!(node.apparent_size, 1 << 20);

    let top = index
        .select(
            rec.id,
            &NodeFilter {
                kind: KindFilter::File,
                limit: 3,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(top[0].apparent_size, 1 << 20, "largest file first");
    assert!(top.windows(2).all(|w| w[0].disk_size >= w[1].disk_size));

    let under = index
        .select(
            rec.id,
            &NodeFilter {
                under: Some(fx.root().join("d050")),
                limit: 1000,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(under.len(), 100);
    assert!(under.iter().all(|n| n.path.contains("/d050/")));

    let kids = index
        .children(index.root_node(rec.id).unwrap().unwrap().id, 500)
        .unwrap();
    assert_eq!(kids.len(), 104);
    let root = index.root_node(rec.id).unwrap().unwrap();
    assert_eq!(
        kids.iter().map(|k| k.disk_size).sum::<u64>() + dir_self(&fx.root()),
        root.disk_size
    );
}

fn dir_self(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(p).unwrap().blocks() * 512
}

#[test]
fn old_scans_are_compacted_then_dropped() {
    let fx = fixture();
    let state = tempfile::tempdir().unwrap();
    let db = db_in(state.path());
    let first = scan_into(&fx.root(), &db, 2);
    let second = scan_into(&fx.root(), &db, 2);
    let index = Index::open(&db).unwrap();
    let count = |id: i64| -> i64 {
        index
            .connection()
            .query_row("SELECT COUNT(*) FROM nodes WHERE scan_id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert!(count(second.id) > 10_000, "newest scan keeps every file");
    assert!(count(first.id) < 200, "older scan keeps directories only");
    let third = scan_into(&fx.root(), &db, 2);
    assert_eq!(count(first.id), 0, "scans beyond keep are dropped");
    assert!(index.scan(first.id).unwrap().is_none());
    assert_eq!(index.latest_scan(None).unwrap().unwrap().id, third.id);
}

#[test]
fn second_scanner_is_refused_while_lock_held() {
    let state = tempfile::tempdir().unwrap();
    let _held = spacehog_core::lock::ScanLock::acquire(state.path(), 42).unwrap();
    let err = spacehog_core::lock::ScanLock::acquire(state.path(), 43).unwrap_err();
    assert!(matches!(
        err,
        spacehog_core::Error::ScanInProgress { scan_id: 42, .. }
    ));
}
