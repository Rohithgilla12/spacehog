# Changelog

## Unreleased

### M5 cleanup and safety (core and CLI)

- Executor in core shared by every surface: re-validates each item, re-stats it (inode, type, and size must match the index), then trashes or deletes it and journals the outcome to SQLite and `journal.jsonl`.
- Agents can execute only approved proposals, or pending trash proposals whose every item is in `mcp.auto_approve`. Permanent deletes always need approval, and the confirm string must equal the proposal total.
- On macOS, trashing is a rename into `~/.Trash`, so `undo` can put items back; the system trash API cannot list or restore there.
- Symlinks are proposed and deleted as links; their targets are never resolved.
- The index subtracts freed bytes immediately and adds them back on undo.
- `spacehog clean` (with `--dry-run`, `--permanent`, `--yes`) and `spacehog undo`.

### M2 categoriser

- Rule engine with embedded default rules covering JS, Python, Go, Rust, JVM, Apple, containers, VCS, macOS app data, and user content. Users extend or override it with `~/.config/spacehog/rules.toml`.
- Fallback rules claim a subtree but keep descending, and per-claim exclusive sizes keep bucket totals exact.
- Staleness measured from the owning project's manifests and `.git`, safety classes with `dist/` promotion, and regen hints chosen from lockfiles.
- `spacehog summary`, `spacehog explain`, `spacehog rules`, and `spacehog top --category/--stale/--safety`.
- Proposal model in core (`propose`, `approve`, `reject`) with path validation shared by every surface.

### M1 scanner and index

- Parallel walker with hardlink dedupe, no symlink following, one-filesystem by default, EACCES nodes, and iCloud dataless detection.
- SQLite index in WAL mode with pre-order ids, so subtree queries are id-range scans.
- `spacehog scan`, `spacehog top`, and `spacehog status`, all with `--json`.

### M0 scaffold

- Cargo workspace with `core`, `tui`, `mcp`, and `cli` crates.
- CI runs fmt, clippy, tests, and a release build on macOS and Linux.
- `spacehog` prints the big-text splash.
