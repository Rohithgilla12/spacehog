#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use spacehog_core::Index;
use spacehog_core::categorise::Categoriser;
use spacehog_core::engine::{ScanRequest, run_scan};
use spacehog_core::rules::RuleSet;
use spacehog_core::scan::Progress;

pub fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'.'; bytes]).unwrap();
}

pub fn age(path: &Path, days: u64) {
    let t = SystemTime::now() - Duration::from_secs(days * 86_400);
    fs::File::open(path).unwrap().set_modified(t).unwrap();
}

/// A fake home with one example of every rule family.
pub fn home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let h = fs::canonicalize(dir.path()).unwrap();
    let files: &[(&str, usize)] = &[
        (".npm/_cacache/index", 4000),
        ("Library/Caches/pip/http/a", 4000),
        ("Library/Caches/Homebrew/node.tar.gz", 4000),
        ("Library/Caches/com.apple.Safari/x", 4000),
        ("Library/Caches/SomeApp/blob", 4000),
        (".cache/huggingface/hub/models--org--llm/blobs/w", 9000),
        (".cache/random-tool/x", 4000),
        ("go/pkg/mod/cache/x", 4000),
        (".cargo/registry/cache/x", 4000),
        (".rustup/toolchains/stable-aarch64/lib/x", 4000),
        ("Library/Developer/Xcode/DerivedData/App-abc/x", 4000),
        ("Library/Application Support/Code/CachedData/x", 4000),
        ("Library/Application Support/Code/User/settings.json", 500),
        ("Library/Application Support/Slack/storage/x", 4000),
        (
            "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
            9000,
        ),
        ("Library/Logs/app.log", 4000),
        ("Library/Mail/V10/x", 4000),
        (".Trash/old.txt", 4000),
        ("Downloads/setup.dmg", 9000),
        ("Downloads/notes.txt", 100),
        ("Documents/essay.txt", 100),
        ("Documents/side/package.json", 50),
        ("Documents/side/node_modules/x/index.js", 4000),
        ("Movies/clip.mp4", 9000),
        ("loose.png", 4000),
        ("backup.tar.gz", 4000),
        (
            "code/web/package.json",
            r#"{"scripts":{"build":"vite build"}}"#.len(),
        ),
        ("code/web/pnpm-lock.yaml", 100),
        ("code/web/node_modules/react/index.js", 4000),
        ("code/web/.next/cache/x", 4000),
        ("code/web/dist/app.js", 4000),
        ("code/web/.git/HEAD", 20),
        ("code/web/.git/lfs/objects/x", 4000),
        ("code/web/src/main.ts", 300),
        ("code/rs/Cargo.toml", 100),
        ("code/rs/target/debug/app", 9000),
        ("code/py/pyproject.toml", 100),
        ("code/py/uv.lock", 100),
        ("code/py/.venv/pyvenv.cfg", 50),
        ("code/py/.venv/lib/x.py", 4000),
        ("code/py/pkg/__pycache__/m.pyc", 4000),
        ("code/old/package.json", 50),
        ("code/old/node_modules/x/index.js", 4000),
        ("code/ios/Podfile", 50),
        ("code/ios/Pods/x", 4000),
        ("code/tf/.terraform/providers/x", 4000),
    ];
    for (p, n) in files {
        write(&h.join(p), *n);
    }
    fs::write(
        h.join("code/web/package.json"),
        r#"{"scripts":{"build":"vite build"}}"#,
    )
    .unwrap();
    fs::create_dir_all(h.join("Applications/Tool.app/Contents")).unwrap();
    write(&h.join("Applications/Tool.app/Contents/bin"), 4000);
    for p in [
        "code/old/package.json",
        "code/old/node_modules/x/index.js",
        "code/old/node_modules/x",
        "code/old/node_modules",
        "code/old",
    ] {
        age(&h.join(p), 200);
    }
    (dir, h)
}

pub fn scan(h: &Path, state: &Path) -> (Index, Arc<RuleSet>, i64) {
    let rules = Arc::new(RuleSet::default_rules(h).unwrap());
    let cat = Arc::new(Categoriser::new(rules.clone()));
    let mut req = ScanRequest::new(h);
    req.db = state.join("index.db");
    req.lock_dir = state.to_path_buf();
    req.labeler = Some(cat);
    let rec = run_scan(&req, Arc::new(Progress::default())).unwrap();
    (Index::open(&req.db).unwrap(), rules, rec.id)
}
