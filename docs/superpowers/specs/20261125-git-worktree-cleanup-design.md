# Git worktree cleanup

Date: 2026-11-25. Status: draft for approval.

## Why

Tools such as Claude Code leave a linked git worktree behind for every task (`.claude/worktrees/agent-*`). On this machine three of them hold 10.7 GB. spacehog already frees their `target/` and `node_modules`, but it cannot remove a worktree itself: `refusal()` in `crates/core/src/proposals.rs` refuses any directory holding a `.git`, and a linked worktree has a `.git` file. The user wants to remove whole worktrees, but only once their work is safely merged.

## Goals

- Claim linked worktrees under their own rule, so they show in Categories, `top`, `clean --category`, and MCP like any other rule.
- Before a worktree is proposed and again before it is deleted, require that it has no uncommitted or untracked changes and that its HEAD is merged into the repository's default branch.
- Keep the Trash and undo path: an undone worktree works again.

## Non-goals

- Deleting branches or running `git worktree prune`. Branches cost nothing, and pruning the metadata would break undo.
- Asking GitHub whether a PR merged. spacehog makes no network calls; `crates/cli/tests/no_network.rs` proves it.
- Removing the main checkout, submodules, or orphaned worktrees whose main repository is gone.

## Anti-goals

Losing work. A false "not merged" is acceptable, since spacehog refuses and says why. A false "merged" is not.

## Approaches considered

| | Merged check | Trade-off |
|---|---|---|
| **A (chosen)** | Local git: HEAD is an ancestor of the default branch, or `git merge-tree --write-tree` of HEAD into it yields the default branch's own tree | No network. Catches merge, fast-forward, squash, and rebase merges. Only as fresh as the local default branch, so an unpulled merge is refused (fails safe). |
| B | `gh pr view --json state` | Answers literally "was the PR merged", but breaks the no-network promise and its test, needs `gh` logged in, and covers GitHub only. |
| C | A by default, B behind a config flag | Two code paths, and the promise becomes "no network unless you opt in". |

The user picked A. On `agent-aeedfc134a904fadb` both tests report merged.

## Design

The check sits in `refusal()`, which every surface already calls at propose time (CLI, MCP, the TUI's `d`) and which the executor calls again right before each deletion (`cleanup.rs:255`). The answer can change between scan and cleanup, so it must not be cached in the index.

```
scan ─► categorise ─► index          (claims the worktree, reads no git state)
                                         │
propose ─► refusal() ─► worktree gate ◄──┤
execute ─► refusal() ─► worktree gate ◄──┘   (runs git again, right before trashing)
                            │
                            ├ git missing or main repo gone ─► refuse
                            ├ locked (git worktree lock)      ─► refuse
                            ├ dirty (status --porcelain)      ─► refuse "N uncommitted changes"
                            ├ not merged into default branch  ─► refuse "pull main if its PR merged"
                            └ otherwise                       ─► allowed; executor re-stats and trashes
```

1. **Detection (categoriser).** A new `git-worktree` rule in `rules/default.toml`, category `worktree` in the VCS bucket, safety `review`, `stale = "self"`, `fallback = true` so the `target/` and `node_modules` inside stay separately claimable. It must precede the `project` rule. Today's `contains` matcher cannot tell a `.git` file from a directory or a worktree from a submodule, so the rule needs a new matcher that holds when the directory has a `.git` *file* whose `gitdir:` line points into a `/worktrees/` directory. Submodules point into `/modules/` and stay `project`.
2. **Gate (core).** A new module runs `git -C <worktree>` with `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, and `GIT_NO_LAZY_FETCH=1`, so status writes no index lock and a partial clone never fetches. The default branch is the target of `refs/remotes/origin/HEAD`, falling back to local `main`, then `master`. HEAD counts as merged if either test above passes against the remote-tracking or the local default branch. A detached HEAD is tested the same way.
3. **refusal().** Skip the "contains a .git directory" refusal for `git-worktree` items and run the gate instead. The gate's reason strings reach the TUI, CLI, and MCP through the existing rejection paths.
4. **Size.** A proposal item's size is the claim's `excl_disk`. For a worktree that is only its source files, about 5% of what trashing it frees. Proposal items, the `max_bytes_per_execution` check, and `bytes_freed` should use the whole subtree. This is a pre-existing bug for every fallback claim (`downloads`, `docker-desktop`), so fix it for all of them.
5. **Undo.** Trashing leaves `.git/worktrees/<name>` in the main repository, which `git worktree list` marks prunable. `spacehog undo` puts the directory back and the link works again. The regen hint says `git worktree add <path> <branch>`, and that `git worktree prune` drops the stale entry once you no longer want undo.

## Constraints

- Core gains its first subprocess. Running `git` reuses the user's own git config (fsmonitor, safe.directory) and adds no dependency. `gix` was the alternative, but its status support is younger and it is a large crate tree.
- `git merge-tree --write-tree` needs git 2.38 or newer (this machine has 2.54). On older git, only the ancestor test runs and squash merges are refused.
- `merge-tree --write-tree` writes one tree object into the main repository's object store. It is unreferenced and `git gc` removes it.
- The no-network test must still pass with a worktree cleanup in it.

## Tests

Core tests against real repositories in a temp dir:

- A clean, merged worktree is accepted, trashed, and restored by undo, and `git worktree list` sees it again.
- A worktree that was squash-merged is accepted.
- A worktree with a modified file, and one with an untracked file, are refused with the change count.
- An unmerged worktree is refused.
- A locked worktree is refused.
- A worktree that turns dirty between propose and execute is skipped at execute.
- A submodule and the main checkout are never claimed as `git-worktree`.
- The proposal size of a worktree equals its whole subtree.

## Left to the builder

- The name and shape of the new rule matcher.
- Where the gate module lives, and whether git calls go through a small trait so tests can fake a missing `git`.
- The exact wording of the refusal reasons, kept in the style of today's reasons.
