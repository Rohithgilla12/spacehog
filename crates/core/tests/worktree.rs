mod common;

use std::fs;
use std::path::PathBuf;

use common::{git, repo_with_worktree};
use spacehog_core::worktree::removal_refusal;

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    wt: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let (repo, wt) = repo_with_worktree(&root, "x");
    Fixture {
        _dir: dir,
        repo,
        wt,
    }
}

impl Fixture {
    fn commit_in_worktree(&self, file: &str, body: &str) {
        fs::write(self.wt.join(file), body).unwrap();
        git(&self.wt, &["add", "."]);
        git(&self.wt, &["commit", "-qm", file]);
    }
}

#[test]
fn clean_merged_worktree_is_removable() {
    let f = fixture();
    f.commit_in_worktree("a.txt", "a");
    git(&f.repo, &["merge", "-q", "--no-ff", "-m", "merge", "wt-x"]);
    assert_eq!(removal_refusal(&f.wt), None);
}

#[test]
fn squash_merged_worktree_is_removable() {
    let f = fixture();
    f.commit_in_worktree("a.txt", "a");
    f.commit_in_worktree("b.txt", "b");
    git(&f.repo, &["merge", "-q", "--squash", "wt-x"]);
    git(&f.repo, &["commit", "-qm", "squash"]);
    assert_eq!(removal_refusal(&f.wt), None);
}

#[test]
fn fresh_worktree_with_no_commits_is_removable() {
    let f = fixture();
    assert_eq!(removal_refusal(&f.wt), None);
}

#[test]
fn detached_head_in_main_is_removable() {
    let f = fixture();
    git(&f.wt, &["checkout", "-q", "--detach"]);
    assert_eq!(removal_refusal(&f.wt), None);
}

#[test]
fn modified_file_refuses() {
    let f = fixture();
    fs::write(f.wt.join("README.md"), "changed\n").unwrap();
    let reason = removal_refusal(&f.wt).unwrap();
    assert!(
        reason.contains("has 1 uncommitted or untracked change;"),
        "{reason}"
    );
}

#[test]
fn untracked_file_refuses() {
    let f = fixture();
    fs::write(f.wt.join("new1.txt"), "1").unwrap();
    fs::write(f.wt.join("new2.txt"), "2").unwrap();
    let reason = removal_refusal(&f.wt).unwrap();
    assert!(
        reason.contains("has 2 uncommitted or untracked changes;"),
        "{reason}"
    );
}

#[test]
fn ignored_build_output_does_not_count() {
    let f = fixture();
    f.commit_in_worktree(".gitignore", "target/\n");
    git(&f.repo, &["merge", "-q", "--no-ff", "-m", "merge", "wt-x"]);
    common::write(&f.wt.join("target/x"), 100);
    assert_eq!(removal_refusal(&f.wt), None);
}

#[test]
fn unmerged_branch_refuses() {
    let f = fixture();
    f.commit_in_worktree("a.txt", "a");
    assert_eq!(
        removal_refusal(&f.wt).as_deref(),
        Some("branch wt-x is not merged into main; pull main if its PR has merged")
    );
}

#[test]
fn locked_worktree_refuses() {
    let f = fixture();
    git(&f.repo, &["worktree", "lock", f.wt.to_str().unwrap()]);
    let reason = removal_refusal(&f.wt).unwrap();
    assert!(reason.contains("locked"), "{reason}");
}

#[test]
fn missing_main_repository_refuses() {
    let f = fixture();
    fs::remove_dir_all(f.repo.join(".git/worktrees/x")).unwrap();
    let reason = removal_refusal(&f.wt).unwrap();
    assert!(reason.contains("main repository is gone"), "{reason}");
}
