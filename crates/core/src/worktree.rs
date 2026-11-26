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
/// nothing uncommitted, nothing ignored that `rebuildable` does not vouch for, and
/// HEAD already in the default branch. Every git call is local, and any state it
/// cannot vouch for refuses.
pub fn removal_refusal(dir: &Path, rebuildable: impl Fn(&Path) -> bool) -> Option<String> {
    let Some(admin) = gitdir(dir).filter(|a| a.exists()) else {
        return Some(
            "is a git worktree whose main repository is gone, so its files may be the only copy"
                .into(),
        );
    };
    if admin.join("locked").exists() {
        return Some("is a locked git worktree (git worktree unlock to allow it)".into());
    }
    let promisor = match git(
        dir,
        &[
            "config",
            "--get-regexp",
            r"^(extensions\.partialclone|remote\..*\.promisor)$",
        ],
    ) {
        Err(_) => {
            return Some("git is not installed, so spacehog cannot check this worktree".into());
        }
        Ok(o) => o.status.success() && !o.stdout.is_empty(),
    };
    if promisor {
        return Some(
            "is in a partial clone, which git may fetch from while checking it; remove it with git"
                .into(),
        );
    }
    let status = match git(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
    ) {
        Err(e) => return Some(format!("git status failed: {e}")),
        Ok(o) if !o.status.success() => {
            return Some(format!("git status failed: {}", first_line(&o.stderr)));
        }
        Ok(o) => Status::parse(&o.stdout),
    };
    if status.changes > 0 {
        return Some(format!(
            "has {} uncommitted or untracked change{}; commit or discard them first",
            status.changes,
            plural(status.changes)
        ));
    }
    let unvouched: Vec<&String> = status
        .ignored
        .iter()
        .filter(|p| !p.ends_with(".DS_Store") && !rebuildable(&dir.join(p)))
        .collect();
    if let Some(first) = unvouched.first() {
        return Some(format!(
            "has {} ignored file{} no rule can rebuild, such as {first}; move or delete them first",
            unvouched.len(),
            plural(unvouched.len())
        ));
    }
    if let Some(hidden) = hidden_from_status(dir) {
        return Some(format!(
            "has files git status cannot see (assume-unchanged or skip-worktree), such as {hidden}"
        ));
    }
    if let Some(sub) = initialised_submodule(dir) {
        return Some(format!(
            "has submodule {sub} checked out, whose ignored and hidden files spacehog cannot check; remove it with git"
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

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `git status --porcelain=v1 -z`, split into real changes and ignored paths.
struct Status {
    changes: usize,
    ignored: Vec<String>,
}

impl Status {
    fn parse(raw: &[u8]) -> Self {
        let mut s = Status {
            changes: 0,
            ignored: Vec::new(),
        };
        let mut entries = raw.split(|&b| b == 0).filter(|e| e.len() > 3);
        while let Some(e) = entries.next() {
            let (code, path) = e.split_at(3);
            if code.starts_with(b"!!") {
                let path = String::from_utf8_lossy(path);
                s.ignored.push(path.trim_end_matches('/').to_string());
                continue;
            }
            s.changes += 1;
            if matches!(code[0], b'R' | b'C') {
                entries.next();
            }
        }
        s
    }
}

/// A tracked file whose edits `git status` skips: `ls-files -v` tags
/// assume-unchanged with a lowercase letter and skip-worktree with `S`.
fn hidden_from_status(dir: &Path) -> Option<String> {
    let out = git(dir, &["ls-files", "-v", "-z"]).ok()?;
    if !out.status.success() {
        return Some("(git ls-files failed)".into());
    }
    out.stdout
        .split(|&b| b == 0)
        .filter(|e| e.len() > 2)
        .find(|e| e[0].is_ascii_lowercase() || e[0] == b'S')
        .map(|e| String::from_utf8_lossy(&e[2..]).into_owned())
}

/// A submodule with files on disk. Status and ls-files stop at its gitlink,
/// so its own ignored and skip-worktree files would go unseen.
fn initialised_submodule(dir: &Path) -> Option<String> {
    let Ok(out) = git(dir, &["ls-files", "--stage", "-z"]) else {
        return Some("(git ls-files failed)".into());
    };
    if !out.status.success() {
        return Some("(git ls-files failed)".into());
    }
    out.stdout
        .split(|&b| b == 0)
        .filter(|e| e.starts_with(b"160000 "))
        .filter_map(|e| e.splitn(2, |&b| b == b'\t').nth(1))
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .find(|p| fs::read_dir(dir.join(p)).is_ok_and(|mut d| d.next().is_some()))
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

/// Variables that would point git at another repository than `dir`'s, which `-C` does not override.
const REPO_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
];

/// No prompts, no index lock, no lazy fetch, and no inherited repository.
fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    let mut cmd = Command::new("git");
    for var in REPO_ENV {
        cmd.env_remove(var);
    }
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null())
        .output()
}
