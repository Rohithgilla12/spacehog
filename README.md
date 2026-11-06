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

## Performance

Measured on an M-series MacBook with a warm cache (2026-11-06):

| Tree | Files | Walk | Categorise | Index write | Peak RSS | `top --json` |
|---|---|---|---|---|---|---|
| projects folder | 1.74M | 9.6 s | 0.33 s | 4.4 s | 669 MB | 7 ms |
| whole `$HOME` | 15.6M | 99 s | 2.3 s | 55 s | 5.2 GB (before the label palette) | 18 ms |

`other` was 8.5% of bytes on that home directory.

## Licence

MIT
