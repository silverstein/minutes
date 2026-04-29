# MCP Live Events Host Compatibility - 2026-04-29

Scope: GitHub #194 / `minutes-l5sa.5`.

## Result

`minutes://events/live` is compatible with the local Claude Desktop and Codex
CLI MCP server configurations on this machine.

Both configured hosts point at the repo-local server:

```text
node /Users/silverbook/Sites/minutes/crates/mcp/dist/index.js
```

Host-config smoke results:

| Host config | Subscribe | Update notification | Cursor read | Reconnect cursor |
| --- | --- | --- | --- | --- |
| Claude Desktop `~/Library/Application Support/Claude/claude_desktop_config.json` | pass | pass | `minutes://events/live?since_seq=0&limit=10` returned appended seq `1` | `1` |
| Codex CLI `~/.codex/config.toml` `[mcp_servers.minutes]` | pass | pass | `minutes://events/live?since_seq=0&limit=10` returned appended seq `1` | `1` |

The smoke harness starts the exact command from each host config over stdio,
subscribes to `minutes://events/live`, appends a local event in an isolated
temporary `HOME`, waits for `notifications/resources/updated`, then reads
`minutes://events/live?since_seq=0&limit=10` and verifies the reconnect cursor.

Command:

```bash
node crates/mcp/test/live_events_host_compat.mjs
```

Output from 2026-04-29:

```json
{
  "checked_at": "2026-04-29T16:26:16.591Z",
  "results": [
    {
      "host": "claude-desktop-config",
      "status": "passed",
      "command": "node",
      "args": [
        "/Users/silverbook/Sites/minutes/crates/mcp/dist/index.js"
      ],
      "subscribed_uri": "minutes://events/live",
      "notification_uri": "minutes://events/live",
      "appended_seq": 1,
      "read_uri": "minutes://events/live?since_seq=0&limit=10",
      "reconnect_cursor": 1
    },
    {
      "host": "codex-cli-config",
      "status": "passed",
      "command": "node",
      "args": [
        "/Users/silverbook/Sites/minutes/crates/mcp/dist/index.js"
      ],
      "subscribed_uri": "minutes://events/live",
      "notification_uri": "minutes://events/live",
      "appended_seq": 1,
      "read_uri": "minutes://events/live?since_seq=0&limit=10",
      "reconnect_cursor": 1
    }
  ]
}
```

Additional live host evidence:

- `codex mcp get minutes` shows the Codex CLI host has `minutes` enabled as a
  stdio server with the same `node .../crates/mcp/dist/index.js` command.
- The current Codex MCP host could call `mcp__minutes__.get_status` and received
  `No recording in progress.`

## Honest Gaps

- This is a host-config protocol smoke, not a GUI click-through inside Claude
  Desktop's chat surface. It proves the exact configured server command accepts
  `resources/subscribe`, emits `notifications/resources/updated`, and supports
  cursor reads over the same MCP SDK protocol that the hosts use.
- `opencode mcp list` is currently blocked locally by OpenCode's own sqlite
  error: `Failed to run the query 'PRAGMA wal_checkpoint(PASSIVE)'`. OpenCode
  host-level proof should be retried after that local database issue is fixed.
- Cursor CLI is installed, but there was no local non-interactive MCP inspection
  path comparable to `codex mcp get minutes`; Cursor was not counted for #194
  closure evidence.
- Cline was not found as an installed local host during this pass.

## Regression Surface

Automated coverage remains in `crates/mcp/src/index.test.ts`:

- URI parsing for `minutes://events/live` and `minutes://events/live?since_seq=N`
- reconnect cursor construction
- MCP client subscription with `notifications/resources/updated`

The host-config smoke in `crates/mcp/test/live_events_host_compat.mjs` should be
run before closing future subscription regressions that depend on real stdio
server startup behavior.
