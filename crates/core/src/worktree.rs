use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The admin directory a worktree's `.git` file points at, resolved against `dir`.
pub fn gitdir(dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dir.join(".git")).ok()?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    (!target.is_empty()).then(|| dir.join(target))
}

/// True when `dir` is a linked worktree. A submodule's `.git` file points into
/// `.git/modules/` instead, and its admin dir has no `commondir`. A missing admin
/// dir still counts, so an orphaned worktree is refused by name rather than protected silently.
pub fn is_linked(dir: &Path) -> bool {
    let Some(admin) = gitdir(dir) else {
        return false;
    };
    let under_worktrees = admin
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "worktrees");
    under_worktrees && (!admin.exists() || admin.join("commondir").is_file())
}

/// Why the worktree at `dir` may not be removed, or None when its work is safe:
/// nothing uncommitted, and HEAD already in the default branch. Every git call is
/// local, and any state it cannot vouch for refuses.
pub fn removal_refusal(dir: &Path) -> Option<String> {
    let Some(admin) = gitdir(dir).filter(|a| a.exists()) else {
        return Some(
            "is a git worktree whose main repository is gone, so its files may be the only copy"
                .into(),
        );
    };
    if admin.join("locked").exists() {
        return Some("is a locked git worktree (git worktree unlock to allow it)".into());
    }
    let status = match git(
        dir,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    ) {
        Err(_) => {
            return Some("git is not installed, so spacehog cannot check this worktree".into());
        }
        Ok(o) if !o.status.success() => {
            return Some(format!("git status failed: {}", first_line(&o.stderr)));
        }
        Ok(o) => o.stdout,
    };
    let changes = String::from_utf8_lossy(&status).lines().count();
    if changes > 0 {
        let s = if changes == 1 { "" } else { "s" };
        return Some(format!(
            "has {changes} uncommitted or untracked change{s}; commit or discard them first"
        ));
    }
    let bases = default_bases(dir);
    let Some(first) = bases.first() else {
        return Some("has no main or master branch to check it was merged into".into());
    };
    if bases.iter().any(|b| merged_into(dir, b)) {
        return None;
    }
    let branch = stdout_of(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|b| b != "HEAD")
        .unwrap_or_else(|| "detached HEAD".into());
    Some(format!(
        "branch {branch} is not merged into {first}; pull {first} if its PR has merged"
    ))
}

/// The default branch, local first, then its remote-tracking ref. Prefers what
/// `origin/HEAD` names, and otherwise whichever of main and master exist.
fn default_bases(dir: &Path) -> Vec<String> {
    let names: Vec<String> = match stdout_of(
        dir,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        Some(remote) => {
            let short = remote
                .strip_prefix("origin/")
                .unwrap_or(&remote)
                .to_string();
            vec![short, remote]
        }
        None => ["main", "master", "origin/main", "origin/master"]
            .map(String::from)
            .to_vec(),
    };
    names
        .into_iter()
        .filter(|n| {
            git(
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{n}^{{commit}}"),
                ],
            )
            .is_ok_and(|o| o.status.success())
        })
        .collect()
}

/// HEAD is in `base` by ancestry (merge or fast-forward), or merging it into
/// `base` would change nothing (squash or rebase merges).
fn merged_into(dir: &Path, base: &str) -> bool {
    let ancestor =
        git(dir, &["merge-base", "--is-ancestor", "HEAD", base]).is_ok_and(|o| o.status.success());
    if ancestor {
        return true;
    }
    let Some(merged) = stdout_of(dir, &["merge-tree", "--write-tree", base, "HEAD"]) else {
        return false;
    };
    let base_tree = stdout_of(dir, &["rev-parse", &format!("{base}^{{tree}}")]);
    merged
        .lines()
        .next()
        .is_some_and(|t| Some(t) == base_tree.as_deref())
}

fn stdout_of(dir: &Path, args: &[&str]) -> Option<String> {
    let o = git(dir, args).ok().filter(|o| o.status.success())?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("no output")
        .to_string()
}

/// No prompts, no index lock, and no lazy fetch from a partial clone's promisor.
fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null())
        .output()
}
