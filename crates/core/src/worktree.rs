use std::fs;
use std::path::{Path, PathBuf};

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
