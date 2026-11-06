#!/usr/bin/env bash
# Records a cleanup proposal exactly as an MCP agent would: it speaks
# JSON-RPC to `spacehog mcp` over stdio and calls `propose_cleanup`.
#
#   scripts/agent-propose.sh "rationale" PATH...
#
# SPACEHOG_BIN picks the binary (default: spacehog on PATH). AGENT_NAME sets
# the client name the TUI shows (default: claude-code). Nothing is deleted:
# the proposal waits for approval in the TUI.
set -euo pipefail

bin=${SPACEHOG_BIN:-spacehog}
agent=${AGENT_NAME:-claude-code}
rationale=$1
shift

json() { python3 -c 'import json, sys; print(json.dumps(sys.argv[1:] if len(sys.argv) > 2 else sys.argv[1]))' "$@"; }
paths=$(python3 -c 'import json, sys; print(json.dumps(sys.argv[1:]))' "$@")

{
  printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":$(json "$agent"),\"version\":\"1\"}}}"
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"propose_cleanup\",\"arguments\":{\"paths\":$paths,\"rationale\":$(json "$rationale")}}}"
  sleep 2
} | "$bin" mcp
