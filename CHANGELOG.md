# Changelog

## Unreleased

### M4 MCP server

- `spacehog mcp` serves `status`, `scan`, `summary`, `top`, `query`, `explain`, `propose_cleanup`, `list_proposals`, and `rules` over stdio, with `spacehog://summary` and `spacehog://proposals` resources and a `free-space` prompt.
- Agents can only propose. Proposals record the client's name and wait for a human to approve them unless `mcp.auto_approve` covers every item.
- Caller-fixable failures come back as tool errors the agent can read; logs go to `~/.local/state/spacehog/mcp.log`, never stdout.
- `spacehog mcp install` prints the config for Claude Code, Claude Desktop, and Cursor, and `--write` merges it into the Desktop or Cursor file after backing it up.

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
