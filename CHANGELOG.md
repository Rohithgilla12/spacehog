# Changelog

## Unreleased

### M5–M7 TUI: Cleanup, Treemap, Categories, Changes, and polish

- Cleanup view with a Queue tab and a Proposals tab. Each item shows its size, bucket, safety class, regen hint, and why it is safe. `enter` approves an agent's proposal, `x` rejects it with a reason the agent reads back, `D` cleans now, `m` switches between Trash and permanent, and `u` undoes the last cleanup.
- `D` re-validates the queue through `propose`, lists anything refused and why, asks about each `review` item, and in permanent mode waits until you type the exact total. The executor runs on its own thread; per-item results stream in as it journals them and a bytes-freed counter counts up.
- `d` refuses protected items everywhere (Overview, Explorer, Treemap, Categories) and says why. Proposals refresh every 500 ms while Cleanup is open.
- Treemap view: a squarified map on a half-block canvas with nested levels (`+`/`-`), labels on tiles of 8x2 cells or more, bucket colours, hover and selection, and a 150 ms zoom on `enter`/click and `backspace`. The Explorer preview draws through the same code.
- Categories view: rules grouped by bucket with totals, item counts, and stale bytes; `enter` lists every matching path, `space`/`a` select, `s` shows only stale items, `d` queues the selection. The footer names the mix of safety classes.
- Changes view (`spacehog --changes`): growth by bucket, the 30 paths that grew and shrank most, the kept scans, and `[`/`]` to pick an older baseline. With one scan kept it says to rescan later.
- `theme = "light"` in config, `NO_COLOR` honoured with no colour attributes at all, mouse support (click to drill in, select, filter by donut segment or legend row; scroll wheel; treemap hover), and every view usable at 80x24.
- Visual redesign: a solid half-block donut, headline figures for the total and what is safe to free (with its stale share), a stacked bar of safety classes, wide bars on a track in every list, an Explorer whose columns line up at any depth, and a full-screen scan view with a files/s area chart and the largest folders as they finish.
- Cleanup hardening from review: `esc` at the permanent-delete prompt rejects the proposal it created, so no one can approve and run it later without typing the total; the finished screen closes with `esc`, so a second `enter` cannot approve the next proposal; a run streams only its own journal rows; and quitting during a cleanup waits for it to finish instead of abandoning an item mid-move.
- The Overview's file count groups thousands.
- New colour scheme, chosen from three rendered candidates: bucket colours group by kind (cool hues for caches, builds, and packages you can rebuild; warm hues for your own files; grey for anything no rule claims) on a painted charcoal background. Safety classes, buckets, and interface chrome no longer share colours. The light theme uses the same bucket colours, darkened for white. All GIFs are re-recorded.
- `scripts/demo-tree.sh` builds the synthetic home the GIFs record; `scripts/agent-propose.sh` files a proposal over MCP the way an agent would; `scripts/screenshots.sh` captures every view of an index with vhs.

### M6 scan diffing (core and CLI)

- `spacehog changes [--since 7d|<scan id>]` reports growth per bucket and the paths that grew or shrank.
- The diff only descends into directories whose totals moved, and reports a directory only when no single child explains 80% of its change, so growth points at `~/.cache/huggingface`, not `~`.
- Compacted older scans are compared like for like: small unclaimed files are ignored on both sides.

### M5 cleanup and safety (core and CLI)

- Executor in core shared by every surface: re-validates each item, re-stats it (inode, type, and size must match the index), then trashes or deletes it and journals the outcome to SQLite and `journal.jsonl`.
- Agents can execute only approved proposals, or pending trash proposals whose every item is in `mcp.auto_approve`. Permanent deletes always need approval, and the confirm string must equal the proposal total.
- On macOS, trashing is a rename into `~/.Trash`, so `undo` can put items back; the system trash API cannot list or restore there.
- Symlinks are proposed and deleted as links; their targets are never resolved.
- The index subtracts freed bytes immediately and adds them back on undo.
- `spacehog clean` (with `--dry-run`, `--permanent`, `--yes`) and `spacehog undo`.

### M4 MCP server

- M5/M6 additions: `execute_cleanup` (approved proposals only, or trash proposals fully covered by `mcp.auto_approve`; `confirm` must equal the total), `undo`, and `changes`.

- `spacehog mcp` serves `status`, `scan`, `summary`, `top`, `query`, `explain`, `propose_cleanup`, `list_proposals`, and `rules` over stdio, with `spacehog://summary` and `spacehog://proposals` resources and a `free-space` prompt.
- Agents can only propose. Proposals record the client's name and wait for a human to approve them unless `mcp.auto_approve` covers every item.
- Caller-fixable failures come back as tool errors the agent can read; logs go to `~/.local/state/spacehog/mcp.log`, never stdout.
- `spacehog mcp install` prints the config for Claude Code, Claude Desktop, and Cursor, and `--write` merges it into the Desktop or Cursor file after backing it up.

### M3 TUI: Overview, Explorer, and scan screen

- Overview: disk gauge, total scanned, a braille donut of rollup buckets with a selectable legend, and the 15 largest categorised items with bucket-coloured bars, staleness badges, and safety glyphs. `enter` opens an item in Explorer and `d` queues it for cleanup.
- Explorer: a lazily loaded tree sorted by size, with bars, percentages, bucket tags, stale badges, a breadcrumb, an `i` panel explaining the rule and safety, and a `p` treemap preview. `o` reveals in Finder and `y` copies the path.
- Scan screen: a live modal with counters, phase, a files/s sparkline, and elapsed time. `esc` keeps the scan running in the background and the views reload when it finishes.
- First run shows the splash and a root picker. Global keys cover help, a fuzzy command palette, filter, rescan, and toggles for apparent size, stale-only, and hidden files. Pending agent proposals show in the status strip.

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
