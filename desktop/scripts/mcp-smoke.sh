#!/usr/bin/env bash
set -euo pipefail

PORT="${SLINT_MCP_PORT:-9315}"
curl -fsS -X POST "http://127.0.0.1:${PORT}/mcp" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
  | grep -q '"list_windows"'
