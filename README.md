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

## Licence

MIT
