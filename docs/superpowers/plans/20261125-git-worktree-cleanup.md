# Git Worktree Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let spacehog remove a linked git worktree once it has no uncommitted changes and its HEAD is merged into the default branch.

**Architecture:** A new `git-worktree` rule claims linked worktrees at scan time from their `.git` file. A new `worktree` module runs local `git` to decide whether one is safe to remove, and `refusal()` calls it, so the check runs at propose time and again right before the executor trashes the item. Proposal items start counting a claim's whole subtree.

**Tech Stack:** Rust 2024 workspace, `spacehog-core`, `std::process::Command` running the system `git` (2.38+ for `merge-tree --write-tree`), SQLite index, `tempfile` in tests.

**Spec:** `docs/superpowers/specs/20261125-git-worktree-cleanup-design.md`

## Global Constraints

- No network. Every git call sets `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, and `GIT_NO_LAZY_FETCH=1`, uses `stdin(Stdio::null())`, and never runs `fetch`, `pull`, `ls-remote`, or `gh`.
- No new crate dependencies.
- Never delete branches and never run `git worktree prune` or `git worktree remove`. Trashing goes through the existing executor.
- Rule id `git-worktree`, category `worktree` mapped to bucket `vcs`, label `Git worktree`, safety `review`, `stale = "self"`, `fallback = true`.
- A false "merged" is a data-loss bug. Every uncertain state (git missing, git error, no default branch) refuses.
- `just check` (fmt-check, clippy with `-D warnings`, tests) passes after every task.
- Comments only for a non-obvious why. No banner comments.

## Review Focus

1. A worktree whose `target/` was rebuilt after the scan. Expect a skip with the existing "changed since it was indexed; rescan" reason, never a partial delete. Pinned in Task 4.
2. A path inside a worktree (its `src/`), proposed directly. Expect refusal, because the gate only checks worktree roots. Pinned in Task 4.
3. A worktree on a detached HEAD whose commit is in `main`. Expect removable. Pinned in Task 3.
4. A worktree created from `main` with no commits of its own. Expect removable, since nothing would be lost. Pinned in Task 3.
5. A worktree whose `.git` file uses a relative `gitdir:` (git 2.48+ `worktree.useRelativePaths`). Expect it to be detected and checked like an absolute one. Pinned in Task 2.

---

### Task 1: Proposals count a claim's whole subtree

**Files:**
- Modify: `crates/core/src/report.rs` (`impl Item`)
- Modify: `crates/core/src/proposals.rs` (`refusal`)
- Test: `crates/core/tests/proposals.rs`

**Interfaces:**
- Produces: `Item::for_cleanup(rules: &RuleSet, n: &NodeInfo) -> Item`, the same as `from_node` but with `size = n.disk_size`, `apparent_size = n.apparent_size`, and `size_human` to match. `refusal()` returns this item, so `propose`, `max_bytes_per_execution`, the stored `proposal_items.size`, and `bytes_freed` all use whole-subtree sizes.

- [ ] **Step 1: Write the failing test** `proposing_a_fallback_claim_counts_its_whole_subtree`. Scan `common::home()`, propose `Downloads` as `Actor::User`, and assert `r.total_bytes == index.find_in_latest(&h.join("Downloads"))?.unwrap().disk_size`. Assert too that it is greater than that node's `excl_disk`, since `setup.dmg` is a nested `installers` claim.
- [ ] **Step 2: Run** `cargo test -p spacehog-core --test proposals proposing_a_fallback_claim`. Expect FAIL, with the total equal to `excl_disk`.
- [ ] **Step 3: Implement** `Item::for_cleanup` and use it in `refusal()` in place of `Item::from_node`.
- [ ] **Step 4: Run** `just check`. Expect PASS. If an existing test pinned an `excl` total, update it only when the path is a fallback claim, and say so in the commit body.
- [ ] **Step 5: Commit** with `fix(core): count a fallback claim's whole subtree in proposals`.

### Task 2: Detect linked worktrees

**Files:**
- Modify: `crates/core/src/rules.rs` (`RuleDef`)
- Modify: `crates/core/src/categorise.rs` (`Ctx::rule_holds`, `label_tree`)
- Modify: `crates/core/rules/default.toml` (category and rule, directly above `[[rule]] id = "project"`)
- Modify: `crates/core/tests/common/mod.rs`
- Test: `crates/core/tests/rules.rs`

**Interfaces:**
- Produces: `RuleDef.worktree: bool` (`#[serde(default)]`, TOML key `worktree`). It holds when the directory has a child named `.git` of kind `NodeKind::File` whose first line is `gitdir: <p>` and `<p>` has a path component `worktrees` directly under a directory named `.git`. A relative `<p>` is resolved against the worktree directory. A submodule's `gitdir:` points into `.git/modules/` and does not match.
- Produces, for tests: `common::git(dir: &Path, args: &[&str]) -> String` runs `git -c user.name=t -c user.email=t@example.com -c init.defaultBranch=main -c commit.gpgsign=false` and panics on failure. `common::repo_with_worktree(root: &Path, name: &str) -> (PathBuf, PathBuf)` creates `root/repo` with one commit, then `git worktree add -b wt-<name> root/repo/.claude/worktrees/<name>`, and returns `(repo, worktree)`.
- Rule TOML, exactly:
  ```toml
  [[rule]]
  id = "git-worktree"
  category = "worktree"
  label = "Git worktree"
  name = ["*"]
  worktree = true
  safety = "review"
  regen = "git worktree add <path> <branch>; git worktree prune drops the stale entry once you no longer need undo"
  stale = "self"
  fallback = true
  ```
  Add `worktree = "vcs"` under `[categories]`.

- [ ] **Step 1: Write the failing tests.**
  - `linked_worktree_is_claimed_and_its_build_output_stays_separate`. Use `repo_with_worktree`, write `<wt>/Cargo.toml` and `<wt>/target/debug/app` (9000 bytes), and scan the root. Assert that the worktree's `rule_id` is `git-worktree`, its safety is `Review`, `<wt>/target` has `cargo-target`, and `repo` keeps `project`.
  - `submodule_git_file_is_not_a_worktree`. Write `root/sub/.git` as `gitdir: ../.git/modules/sub` beside a `root/.git/modules/sub/HEAD`, then assert that `sub` has `project`.
  - `relative_gitdir_worktree_is_claimed`. Run `repo_with_worktree`, rewrite the `.git` file to the relative path from the worktree to `repo/.git/worktrees/<name>`, then assert `git-worktree`.
- [ ] **Step 2: Run** `cargo test -p spacehog-core --test rules worktree`. Expect FAIL, either because `worktree` is an unknown field or because the rule id is `project`.
- [ ] **Step 3: Implement.** Add the field and the TOML. In `rule_holds`, pass the node's `&Path` and read only the first line of `<path>/.git` when `rule.def.worktree` is set and the `.git` child is a file.
- [ ] **Step 4: Run** `just check`. Expect PASS, including `summary_accounts_for_every_byte_once`.
- [ ] **Step 5: Commit** with `feat(core): claim linked git worktrees under a git-worktree rule`.

### Task 3: The worktree removal gate

**Files:**
- Create: `crates/core/src/worktree.rs`
- Modify: `crates/core/src/lib.rs` (`pub mod worktree;`)
- Test: `crates/core/tests/worktree.rs`

**Interfaces:**
- Consumes: `common::git` and `common::repo_with_worktree` from Task 2.
- Produces: `pub fn removal_refusal(dir: &Path) -> Option<String>`. It returns `None` when the worktree at `dir` is safe to remove, and otherwise the reason. It runs these checks in order and stops at the first that fails:
  1. `<dir>/.git` is unreadable, or its `gitdir` does not exist: `"is a git worktree whose main repository is gone, so its files may be the only copy"`
  2. `<gitdir>/locked` exists: `"is a locked git worktree (git worktree unlock to allow it)"`
  3. `git` cannot be spawned: `"git is not installed, so spacehog cannot check this worktree"`
  4. `git status --porcelain=v1 -z --untracked-files=normal` lists N > 0 entries: `"has {N} uncommitted or untracked change{s}; commit or discard them first"`. Any git failure: `"git status failed: {stderr first line}"`.
  5. Bases are the target of `git symbolic-ref --quiet --short refs/remotes/origin/HEAD` (e.g. `origin/main`) and the local branch of the same short name if it exists. Without `origin/HEAD`, the bases are whichever of `main`, `master`, `origin/main`, `origin/master` resolve via `git rev-parse --verify --quiet`. With none: `"has no main or master branch to check it was merged into"`.
  6. HEAD is merged if, for any base, `git merge-base --is-ancestor HEAD <base>` exits 0, or `git merge-tree --write-tree <base> HEAD` exits 0 and its first stdout line equals `git rev-parse <base>^{tree}`. Otherwise: `"branch {b} is not merged into {base}; pull {base} if its PR has merged"`, where `{b}` is `git rev-parse --abbrev-ref HEAD` (`detached HEAD` when that prints `HEAD`) and `{base}` is the first base.
- All git calls go through one private `fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output>` that applies the Global Constraints env, so no call site can forget them.

- [ ] **Step 1: Write the failing tests**, one per row. Each builds the worktree with `repo_with_worktree`.

  | Test | Setup | Assert |
  |---|---|---|
  | `clean_merged_worktree_is_removable` | commit in wt, `git merge --no-ff wt-x` in repo | `None` |
  | `squash_merged_worktree_is_removable` | commit in wt, `git merge --squash wt-x` and commit in repo | `None` |
  | `fresh_worktree_with_no_commits_is_removable` | nothing | `None` |
  | `detached_head_in_main_is_removable` | `git checkout --detach` in wt | `None` |
  | `modified_file_refuses` | edit a tracked file | contains `"has 1 uncommitted or untracked change;"` |
  | `untracked_file_refuses` | write two new files | contains `"has 2 uncommitted or untracked changes;"` |
  | `ignored_build_output_does_not_count` | `.gitignore` with `target/` committed and merged, then write `target/x` | `None` |
  | `unmerged_branch_refuses` | commit in wt only | equals `"branch wt-x is not merged into main; pull main if its PR has merged"` |
  | `locked_worktree_refuses` | `git worktree lock` | contains `"locked"` |
  | `missing_main_repository_refuses` | delete `repo/.git/worktrees/x` | contains `"main repository is gone"` |

- [ ] **Step 2: Run** `cargo test -p spacehog-core --test worktree`. Expect FAIL, since the module does not exist.
- [ ] **Step 3: Implement** `removal_refusal` per the Interfaces block.
- [ ] **Step 4: Run** `just check`. Expect PASS.
- [ ] **Step 5: Commit** with `feat(core): check a worktree is clean and merged before removal`.

### Task 4: Wire the gate into refusal, end to end

**Files:**
- Modify: `crates/core/src/proposals.rs` (`refusal`)
- Test: `crates/core/tests/cleanup.rs`

**Interfaces:**
- Consumes: `worktree::removal_refusal` (Task 3) and `RuleDef.worktree` (Task 2).
- Behaviour in `refusal()`, where `is_worktree = rule.def.worktree` for the node's rule:
  - If `is_worktree` and `!node.claim`: `Err("is inside a git worktree; clean the whole worktree instead")`.
  - If `is_worktree`, skip the "contains a .git directory" refusal.
  - After the safety match, if `is_worktree`, return `Err(reason)` when `removal_refusal(path)` gives one.

- [ ] **Step 1: Write the failing tests.** Each scans a temp root holding `repo_with_worktree` with `Actor::User` and `Trash::Rename`.
  - `merged_worktree_is_trashed_and_undo_restores_a_working_checkout`. Merge the branch, propose the worktree, and execute. Assert `Outcome::Trashed` and that the directory is gone. Run `undo`, then assert `git -C <wt> status` succeeds and `git -C repo worktree list --porcelain` names `<wt>`.
  - `dirty_worktree_is_rejected_at_propose`. Assert the rejection reason contains `"uncommitted"`.
  - `worktree_that_turns_dirty_after_propose_is_skipped`. Merge, propose, write an untracked file, then execute. Assert `Outcome::Skipped` with a reason containing `"uncommitted"`, and that the directory still exists.
  - `rebuilt_worktree_is_skipped_as_changed`. Merge, propose, grow `<wt>/target/debug/app`, then execute. Assert `Outcome::Skipped` with a reason containing `"changed since it was indexed"`.
  - `path_inside_a_worktree_is_refused`. Propose `<wt>/src`. Assert the reason equals `"is inside a git worktree; clean the whole worktree instead"`.
  - `main_checkout_is_never_proposable`. Propose `repo`. Assert the reason starts with `"contains a .git directory"`.
- [ ] **Step 2: Run** `cargo test -p spacehog-core --test cleanup worktree`. Expect FAIL, with the worktree refused as `contains a .git directory`.
- [ ] **Step 3: Implement** the `refusal()` changes.
- [ ] **Step 4: Run** `just check`. Expect PASS.
- [ ] **Step 5: Commit** with `feat(core): let cleanup remove clean, merged git worktrees`.

### Task 5: Prove it stays offline, and document it

**Files:**
- Modify: `crates/cli/tests/no_network.rs`
- Modify: `README.md` ("How spacehog decides what is safe")
- Modify: `CHANGELOG.md` (`## Unreleased`)
- Modify: `docs/decisions.md` (new section `## Git worktrees: merged means local git says so`)

- [ ] **Step 1: Extend** `every_command_works_with_the_network_denied`. Build a repo with a merged worktree under `tree` using plain `Command::new("git")` with the same `-c` flags as `common::git`. After `scan`, add `["clean", "--category", "worktree", "--dry-run", "--json"]`, and assert its stdout names the worktree path, so the gate ran inside the sandbox.
- [ ] **Step 2: Run** `cargo test -p spacehog --test no_network`. Expect PASS on macOS.
- [ ] **Step 3: Docs.** The README gets one paragraph saying that a linked worktree is cleaned only when `git status` is clean and its HEAD is in the default branch, checked by local git with no network, and that `git worktree prune` is left to you so undo works. The CHANGELOG gets one bullet. `decisions.md` gets the approaches table from the spec, the fail-safe cost of an unpulled `main`, and the git 2.38 floor.
- [ ] **Step 4: Run** `just check`. Then run `cargo run -p spacehog -- scan ~/github.com/Rohithgilla12/spacehog/.claude/worktrees` followed by `cargo run -p spacehog -- clean --category worktree --dry-run`, and check that `agent-aeedfc134a904fadb` is reported. It has 1 uncommitted change, so expect a rejection naming it. Do not execute.
- [ ] **Step 5: Commit** with `docs: worktree cleanup in README, changelog, and decisions; offline test`.
