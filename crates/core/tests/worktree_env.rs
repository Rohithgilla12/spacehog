mod common;

use std::fs;
use std::path::Path;

use spacehog_core::worktree::removal_refusal;

/// Its own test binary, because it sets process-wide git variables.
#[test]
fn inherited_git_dir_does_not_redirect_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let (_, wt) = common::repo_with_worktree(&root, "x");
    fs::write(wt.join("README.md"), "changed\n").unwrap();
    let other = root.join("other");
    fs::create_dir_all(&other).unwrap();
    common::git(&other, &["init", "-q"]);
    // SAFETY: the only test in this binary, and every git call after this is the gate's.
    unsafe {
        std::env::set_var("GIT_DIR", other.join(".git"));
        std::env::set_var("GIT_WORK_TREE", &other);
    }
    let reason = removal_refusal(&wt, |_: &Path| false).unwrap();
    assert!(reason.contains("uncommitted"), "{reason}");
}
