mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{git, repo_with_worktree};
use spacehog_core::worktree::removal_refusal;

fn no_rebuild(_: &Path) -> bool {
    false
}

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
    assert_eq!(removal_refusal(&f.wt, no_rebuild), None);
}

#[test]
fn squash_merged_worktree_is_removable() {
    let f = fixture();
    f.commit_in_worktree("a.txt", "a");
    f.commit_in_worktree("b.txt", "b");
    git(&f.repo, &["merge", "-q", "--squash", "wt-x"]);
    git(&f.repo, &["commit", "-qm", "squash"]);
    assert_eq!(removal_refusal(&f.wt, no_rebuild), None);
}

#[test]
fn fresh_worktree_with_no_commits_is_removable() {
    let f = fixture();
    assert_eq!(removal_refusal(&f.wt, no_rebuild), None);
}

#[test]
fn detached_head_in_main_is_removable() {
    let f = fixture();
    git(&f.wt, &["checkout", "-q", "--detach"]);
    assert_eq!(removal_refusal(&f.wt, no_rebuild), None);
}

#[test]
fn modified_file_refuses() {
    let f = fixture();
    fs::write(f.wt.join("README.md"), "changed\n").unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
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
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
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
    assert_eq!(
        removal_refusal(&f.wt, |p: &Path| p.ends_with("target")),
        None
    );
}

#[test]
fn unmerged_branch_refuses() {
    let f = fixture();
    f.commit_in_worktree("a.txt", "a");
    assert_eq!(
        removal_refusal(&f.wt, no_rebuild).as_deref(),
        Some("branch wt-x is not merged into main; pull main if its PR has merged")
    );
}

#[test]
fn locked_worktree_refuses() {
    let f = fixture();
    git(&f.repo, &["worktree", "lock", f.wt.to_str().unwrap()]);
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(reason.contains("locked"), "{reason}");
}

#[test]
fn missing_main_repository_refuses() {
    let f = fixture();
    fs::remove_dir_all(f.repo.join(".git/worktrees/x")).unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(reason.contains("main repository is gone"), "{reason}");
}

#[test]
fn ignored_file_no_rule_rebuilds_refuses() {
    let f = fixture();
    f.commit_in_worktree(".gitignore", ".env\ntarget/\n");
    git(&f.repo, &["merge", "-q", "--no-ff", "-m", "merge", "wt-x"]);
    fs::write(f.wt.join(".env"), "SECRET=1").unwrap();
    common::write(&f.wt.join("target/x"), 100);
    let reason = removal_refusal(&f.wt, |p: &Path| p.ends_with("target")).unwrap();
    assert!(
        reason.contains("1 ignored file no rule can rebuild, such as .env"),
        "{reason}"
    );
}

#[test]
fn excluded_ds_store_does_not_count() {
    let f = fixture();
    fs::write(f.repo.join(".git/info/exclude"), ".DS_Store\n").unwrap();
    fs::write(f.wt.join(".DS_Store"), "finder").unwrap();
    assert_eq!(removal_refusal(&f.wt, no_rebuild), None);
}

#[test]
fn assume_unchanged_edit_refuses() {
    let f = fixture();
    git(&f.wt, &["update-index", "--assume-unchanged", "README.md"]);
    fs::write(f.wt.join("README.md"), "hidden edit\n").unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(
        reason.contains("assume-unchanged or skip-worktree"),
        "{reason}"
    );
}

#[test]
fn skip_worktree_edit_refuses() {
    let f = fixture();
    git(&f.wt, &["update-index", "--skip-worktree", "README.md"]);
    fs::write(f.wt.join("README.md"), "hidden edit\n").unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(
        reason.contains("assume-unchanged or skip-worktree"),
        "{reason}"
    );
}

/// Adds a submodule at `lib`, with `lib_ignore` as its own .gitignore, and merges it.
fn with_merged_submodule(f: &Fixture, lib_ignore: &str) {
    let lib = f.repo.parent().unwrap().join("lib");
    fs::create_dir_all(&lib).unwrap();
    git(&lib, &["init", "-q"]);
    fs::write(lib.join("lib.rs"), "fn a() {}\n").unwrap();
    fs::write(lib.join(".gitignore"), lib_ignore).unwrap();
    git(&lib, &["add", "."]);
    git(&lib, &["commit", "-qm", "lib"]);
    git(
        &f.wt,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            lib.to_str().unwrap(),
            "lib",
        ],
    );
    git(&f.wt, &["commit", "-qm", "add lib"]);
    git(&f.repo, &["merge", "-q", "--no-ff", "-m", "merge", "wt-x"]);
}

#[test]
fn dirty_submodule_refuses_even_when_config_ignores_submodules() {
    let f = fixture();
    with_merged_submodule(&f, "");
    git(&f.repo, &["config", "diff.ignoreSubmodules", "all"]);
    fs::write(f.wt.join("lib/lib.rs"), "fn b() {}\n").unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(reason.contains("uncommitted"), "{reason}");
}

#[test]
fn partial_clone_refuses() {
    let f = fixture();
    git(&f.repo, &["config", "remote.origin.promisor", "true"]);
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(reason.contains("partial clone"), "{reason}");
}

#[test]
fn ignored_file_inside_a_submodule_refuses() {
    let f = fixture();
    with_merged_submodule(&f, ".env\n");
    fs::write(f.wt.join("lib/.env"), "SECRET=1").unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(reason.contains("submodule lib"), "{reason}");
}

#[test]
fn skip_worktree_edit_inside_a_submodule_refuses() {
    let f = fixture();
    with_merged_submodule(&f, "");
    let lib = f.wt.join("lib");
    git(&lib, &["update-index", "--skip-worktree", "lib.rs"]);
    fs::write(lib.join("lib.rs"), "fn b() {}\n").unwrap();
    let reason = removal_refusal(&f.wt, no_rebuild).unwrap();
    assert!(reason.contains("submodule lib"), "{reason}");
}
