# spacehog

*Find the hog. Feed it to your agent.*

spacehog is a disk-usage tool with two faces on one engine. The TUI shows where your disk went with treemaps, category donuts, and proportional bars. The MCP server lets an AI agent ask the same engine what is eating the disk, propose a cleanup, and run it once you approve.

It knows what `node_modules`, `target/`, `__pycache__`, Go build caches, Docker images, and Xcode DerivedData are. It knows whether each one can be regenerated and how stale the project that owns it is.

## Status

Pre-release. See [CHANGELOG.md](CHANGELOG.md).

## Install

```sh
cargo install --path crates/cli
```

## Usage

```sh
spacehog            # TUI
spacehog mcp        # MCP server over stdio
spacehog --help     # CLI
```

## Let Claude clean your disk

`spacehog mcp` serves the engine to MCP clients over stdio. Print the config for Claude Code, Claude Desktop, and Cursor:

```sh
spacehog mcp install                          # all three snippets
spacehog mcp install --client cursor --write  # merge into ~/.cursor/mcp.json, backing it up first
```

For Claude Code, run the `claude mcp add spacehog -- <path> mcp` line it prints. `--write` covers Claude Desktop and Cursor, keeping every other server in the file.

The agent gets `status`, `scan`, `summary`, `top`, `query`, `explain`, `propose_cleanup`, `list_proposals`, and `rules`. It can propose a cleanup but cannot run one: you approve proposals in the TUI or with `spacehog proposals approve <id>`, unless you list a safety class in `mcp.auto_approve`. Logs go to `~/.local/state/spacehog/mcp.log`.

[examples/claude-code.md](examples/claude-code.md) is a captured session that frees space after an `ENOSPC`.

## Performance

Measured on an M-series MacBook with a warm cache (2026-11-06):

| Tree | Files | Walk | Categorise | Index write | Peak RSS | `top --json` |
|---|---|---|---|---|---|---|
| projects folder | 1.74M | 9.6 s | 0.33 s | 4.4 s | 669 MB | 7 ms |
| whole `$HOME` | 15.6M | 99 s | 2.3 s | 55 s | 5.2 GB (before the label palette) | 18 ms |

`other` was 8.5% of bytes on that home directory.

## Licence

MIT
