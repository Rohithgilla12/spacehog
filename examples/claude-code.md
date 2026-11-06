# Claude Code and spacehog: freeing space after ENOSPC

A build fails with `ENOSPC: no space left on device`. This walk-through shows what Claude Code
sees when it uses spacehog to find space it can safely reclaim. The agent looks, explains, and
proposes. You approve. Nothing is deleted without you.

## Connect Claude Code

```sh
spacehog mcp install --client claude-code
# Claude Code:
#   claude mcp add spacehog -- /path/to/spacehog mcp
```

Run the printed `claude mcp add` command, then restart Claude Code. The server's log is
`~/.local/state/spacehog/mcp.log`; nothing but protocol frames goes to stdout.

## About this transcript

This is a captured stdio session, not an invented one. It was recorded on 2026-11-06 by driving
`spacehog mcp` (release build of this repository) with hand-written JSON-RPC, one frame per line,
the way Claude Code talks to it. The index came from a `scan` of `~/code`
(1.76M files, 107.2 GB on disk) made through the same server just before, with
`SPACEHOG_DATA_DIR` pointed at a scratch directory.

Requests are shown as sent. For tool calls the response shows `structuredContent`; the server
mirrors the same JSON into the text content block for clients that only read text. To keep the
page readable, arrays longer than two items keep their first two and end with a
`// N more elided` line. Nothing else was edited.

## Transcript

### `initialize`

The server names itself `spacehog` and sends instructions that spell out the approval model before the agent calls anything.

```jsonc
// ->
{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2.1.0"}}}
```

```jsonc
// <- result
{
  "protocolVersion": "2025-06-18",
  "capabilities": {
    "prompts": {},
    "resources": {},
    "tools": {}
  },
  "serverInfo": {
    "name": "spacehog",
    "version": "0.1.0"
  },
  "instructions": "spacehog indexes disk usage and classifies large items by safety: `regenerable` (build output a command recreates), `cache`, `review` (a human should look first), and `protected` (never cleaned).\n\nStart with `status`. If there is no recent scan, call `scan` and poll `status` until it finishes. Use `summary` and `top` to find candidates, preferring regenerable and cache items and stale ones first. Call `explain` on every path before you propose it, and always for `review` items.\n\nYou cannot delete anything. `propose_cleanup` records a proposal; a human approves it in the spacehog TUI or with `spacehog proposals approve <id>`, so tell the user the proposal id and ask them to approve it. Execution needs that approval unless the user has listed every item's safety class in `mcp.auto_approve`; `requires_approval` in the result says which applies.\n\nSizes are bytes with a human-readable string beside them. Paths are absolute."
}
```

### notification `notifications/initialized`

### `status`

The agent starts where the instructions say. The volume is 93.1% full, and the last scan finished with no scan running.

```jsonc
// ->
{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "status", "arguments": {}}}
```

```jsonc
// <- result.structuredContent (isError: false)
{
  "last_scan": {
    "apparent_bytes": 108827830832,
    "apparent_human": "108.8 GB",
    "bytes": 107175428096,
    "bytes_human": "107.2 GB",
    "current_path": null,
    "dirs": 250106,
    "errors": 0,
    "files": 1762269,
    "finished_at": 1793946495,
    "id": 1,
    "pid": 50425,
    "root": "/Users/me/code",
    "started_at": 1793946480,
    "status": "done"
  },
  "pending_proposals": 0,
  "scan_in_progress": null,
  "volumes": [
    {
      "free": 68342188547,
      "free_human": "68.3 GB",
      "mount": "/",
      "total": 994662584320,
      "total_human": "994.7 GB",
      "used": 926320395773,
      "used_human": "926.3 GB",
      "used_pct": 93.1
    },
    {
      "free": 69779456,
      "free_human": "69.8 MB",
      "mount": "/Volumes/OpenCode",
      "total": 856551424,
      "total_human": "856.6 MB",
      "used": 786771968,
      "used_human": "786.8 MB",
      "used_pct": 91.9
    }
    // 1 more elided
  ]
}
```

### `top`

It asks for stale, regenerable dev caches: idle at least 90 days, safety class `regenerable`, largest first.

```jsonc
// ->
{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "top", "arguments": {"category": "dev-caches", "safety": ["regenerable"], "stale_days": 90, "limit": 3}}}
```

```jsonc
// <- result.structuredContent (isError: false)
{
  "items": [
    {
      "apparent_size": 508941305,
      "bucket": "dev-caches",
      "bucket_label": "Dev caches",
      "kind": "dir",
      "last_modified": 1780555183,
      "path": "/Users/me/code/demo-app/apps/marketing/node_modules",
      "project_root": "/Users/me/code/demo-app",
      "regen_hint": "npm i",
      "rule_id": "node-modules",
      "rule_label": "node_modules",
      "safety": "regenerable",
      "size": 509308928,
      "size_human": "509.3 MB",
      "stale_days": 92
    },
    {
      "apparent_size": 1264090,
      "bucket": "dev-caches",
      "bucket_label": "Dev caches",
      "kind": "dir",
      "last_modified": 1784875663,
      "path": "/Users/me/code/demo-app/apps/landing/node_modules",
      "project_root": "/Users/me/code/demo-app",
      "regen_hint": "npm i",
      "rule_id": "node-modules",
      "rule_label": "node_modules",
      "safety": "regenerable",
      "size": 1351680,
      "size_human": "1.4 MB",
      "stale_days": 92
    }
    // 1 more elided
  ],
  "scan_id": 1
}
```

### `explain`

Before proposing anything it explains the biggest hit. The rule, the safety reason, and how staleness was measured all come back, along with the largest children.

```jsonc
// ->
{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "explain", "arguments": {"path": "/Users/me/code/demo-app/apps/marketing/node_modules"}}}
```

```jsonc
// <- result.structuredContent (isError: false)
{
  "children": [
    {
      "apparent_size": 283957683,
      "bucket": "dev-caches",
      "bucket_label": "Dev caches",
      "kind": "dir",
      "last_modified": 1780555183,
      "path": "/Users/me/code/demo-app/apps/marketing/node_modules/.cache",
      "project_root": null,
      "regen_hint": "npm i",
      "rule_id": "node-modules",
      "rule_label": "node_modules",
      "safety": "regenerable",
      "size": 283983872,
      "size_human": "284.0 MB",
      "stale_days": null
    },
    {
      "apparent_size": 202109888,
      "bucket": "dev-caches",
      "bucket_label": "Dev caches",
      "kind": "dir",
      "last_modified": 1778929038,
      "path": "/Users/me/code/demo-app/apps/marketing/node_modules/.remotion",
      "project_root": null,
      "regen_hint": "npm i",
      "rule_id": "node-modules",
      "rule_label": "node_modules",
      "safety": "regenerable",
      "size": 202149888,
      "size_human": "202.1 MB",
      "stale_days": null
    }
    // 2 more elided
  ],
  "claimed_by": "/Users/me/code/demo-app/apps/marketing/node_modules",
  "item": {
    "apparent_size": 508941305,
    "bucket": "dev-caches",
    "bucket_label": "Dev caches",
    "kind": "dir",
    "last_modified": 1780555183,
    "path": "/Users/me/code/demo-app/apps/marketing/node_modules",
    "project_root": "/Users/me/code/demo-app",
    "regen_hint": "npm i",
    "rule_id": "node-modules",
    "rule_label": "node_modules",
    "safety": "regenerable",
    "size": 509308928,
    "size_human": "509.3 MB",
    "stale_days": 92
  },
  "last_scanned": 1793946495,
  "node": {
    "apparent_size": 508941305,
    "atime": 1780484335,
    "bucket": "dev-caches",
    "child_count": 4,
    "claim": true,
    "depth": 4,
    "dev": 16777230,
    "disk_size": 509308928,
    "errno": null,
    "excl_apparent": 508941305,
    "excl_disk": 509308928,
    "file_count": 162,
    "flags": 0,
    "id": 1098642,
    "inode": 484508254,
    "kind": "dir",
    "last_desc": 1098837,
    "max_mtime": 1780555183,
    "mtime": 1780484335,
    "name": "node_modules",
    "nlink": 6,
    "parent_id": 1098612,
    "path": "/Users/me/code/demo-app/apps/marketing/node_modules",
    "project_root": "/Users/me/code/demo-app",
    "regen_hint": "npm i",
    "rule_id": "node-modules",
    "safety": "regenerable",
    "scan_id": 1,
    "stale_days": 92,
    "uid": 501
  },
  "rule": {
    "category": "dev-cache",
    "contains": [],
    "ext": [],
    "fallback": false,
    "icon": "",
    "id": "node-modules",
    "kind": null,
    "label": "node_modules",
    "name": [
      "node_modules"
    ],
    "path": [],
    "promote": null,
    "regen": "npm i",
    "regen_cmd": [
      {
        "cmd": "pnpm i",
        "sibling": "pnpm-lock.yaml"
      },
      {
        "cmd": "yarn",
        "sibling": "yarn.lock"
      }
      // 3 more elided
    ],
    "safety": "regenerable",
    "sibling": [
      "package.json"
    ],
    "stale": "project"
  },
  "safety_reason": "regenerable via rule `node-modules` (node_modules)",
  "staleness": "stale: last activity 92 days ago, measured from its project /Users/me/code/demo-app (manifests, .git/HEAD, sibling files); threshold 90 days"
}
```

### `propose_cleanup`

It proposes both `node_modules` folders and, to show the guard rails, the project root itself. The root is rejected because it holds a `.git` directory. The two caches are accepted into a pending proposal that needs your approval.

```jsonc
// ->
{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "propose_cleanup", "arguments": {"paths": ["/Users/me/code/demo-app/apps/marketing/node_modules", "/Users/me/code/demo-app/apps/landing/node_modules", "/Users/me/code/demo-app"], "rationale": "Disk full (ENOSPC). These node_modules belong to demo-app, untouched for 92 days; `npm i` restores them."}}}
```

```jsonc
// <- result.structuredContent (isError: false)
{
  "accepted": [
    {
      "apparent_size": 1264090,
      "bucket": "dev-caches",
      "bucket_label": "Dev caches",
      "kind": "dir",
      "last_modified": 1784875663,
      "path": "/Users/me/code/demo-app/apps/landing/node_modules",
      "project_root": "/Users/me/code/demo-app",
      "regen_hint": "npm i",
      "rule_id": "node-modules",
      "rule_label": "node_modules",
      "safety": "regenerable",
      "size": 1351680,
      "size_human": "1.4 MB",
      "stale_days": 92
    },
    {
      "apparent_size": 508941305,
      "bucket": "dev-caches",
      "bucket_label": "Dev caches",
      "kind": "dir",
      "last_modified": 1780555183,
      "path": "/Users/me/code/demo-app/apps/marketing/node_modules",
      "project_root": "/Users/me/code/demo-app",
      "regen_hint": "npm i",
      "rule_id": "node-modules",
      "rule_label": "node_modules",
      "safety": "regenerable",
      "size": 509308928,
      "size_human": "509.3 MB",
      "stale_days": 92
    }
  ],
  "mode": "trash",
  "proposal_id": "pe922a022",
  "rejected": [
    {
      "path": "/Users/me/code/demo-app",
      "reason": "contains a .git directory, so it is a project, not a cache"
    }
  ],
  "requires_approval": true,
  "total_bytes": 510660608,
  "total_human": "510.7 MB"
}
```

## Approving

The agent cannot run the cleanup. It reports the proposal id and asks you to approve it:

```sh
spacehog proposals list
spacehog proposals approve pe922a022
```

The TUI shows the same pending proposal. Only safety classes you list in `mcp.auto_approve`
in `~/.config/spacehog/config.toml` skip this step, and `permanent` mode never does.
