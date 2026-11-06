# Changelog

## Unreleased

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
