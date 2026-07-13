# Audit Event Schema

`rust-junosmcp` and `rust-srxmcp` emit structured audit logs for every MCP tool invocation. Each event records the caller, tool, target routers, authorization decision, outcome, and duration. Events are written to stderr (or an optional append-only JSON file) and are machine-parseable for SIEM ingestion.

## Schema

Every audit event has `target="audit"` and the following fields (in order):

| Field | Type | Description |
|-------|------|-------------|
| `correlation_id` | string | Unique request identifier (`req-<nanos>` epoch-based). |
| `caller` | string | Bearer-token name, or `"stdio"` when unauthenticated. |
| `tool` | string | MCP tool name (e.g., `execute_junos_command`, `get_chassis_cluster_status`). |
| `routers` | string | Comma-separated list of target router names (empty for inventory/list tools). |
| `router_count` | u64 | Number of target routers. |
| `action` | string | Stable action category: `read`, `commit`, `add-device`, `upgrade`, `pfe`, `transfer`, `destructive`, etc. |
| `authorization` | enum | Authorization decision: `allowed`, `denied`, or `no_auth` (stdio caller). |
| `result` | enum | Outcome: `ok` (success), `error` (failure), `denied` (authorization rejected), or `unsettled` (client disconnect). |
| `duration_ms` | u64 | Elapsed time from handler entry to drop (milliseconds). |
| `error_kind` | string | Stable error category when `result=error` (e.g., `"error"`, `"timeout"`, `"lease_busy"`). Empty otherwise. |
| `error` | string | Bounded error message when `result=error` (max 512 chars, truncated with `…`). Empty otherwise. |
| `reason` | string | Denial reason when `result=denied` (see below). Empty otherwise. |
| `metadata` | string | Space-separated `key=value` pairs of allowlisted, non-secret tool-specific fields (e.g., `command_count=5 dry_run=true`). Empty if none. |

### Authorization values

- **`allowed`** — caller has required scopes; work proceeds.
- **`denied`** — caller lacks required scopes or context; work refused before execution.
- **`no_auth`** — stdio transport (no bearer token); treated as allowed.

### Result values

- **`ok`** — handler completed successfully.
- **`error`** — handler returned an error (see `error_kind` and `error`).
- **`denied`** — authorization check rejected the request (see `reason`).
- **`unsettled`** — guard dropped without an outcome (client disconnect or cancel).

### Denial reasons

| Reason | Meaning |
|--------|---------|
| `tool_scope` | Token lacks permission for the requested tool. |
| `router_scope` | Token lacks permission for one or more target routers. |
| `inventory_readonly` | Server started with `--inventory-readonly`; inventory mutations refused. |
| `missing_caller_context` | SRX tool invoked without caller context (stdio or unauthenticated HTTP). |

## JSON Event Format

When `--audit-format json` is set, events are emitted as line-delimited JSON. The `tracing` crate's JSON formatter nests field data under a `"fields"` object:

```json
{"timestamp":"2026-07-12T18:32:14.091234Z","level":"INFO","target":"audit","fields":{"correlation_id":"req-1720805534091123456","caller":"ci","tool":"execute_junos_command","routers":"vsrx-lab-01","router_count":1,"action":"read","authorization":"allowed","result":"ok","duration_ms":142,"error_kind":"","error":"","reason":"","metadata":"format=text"},"message":"audit"}
```

### Example: Success

```json
{"timestamp":"2026-07-12T18:32:15.001Z","level":"INFO","target":"audit","fields":{"correlation_id":"req-1720805535001000000","caller":"automation","tool":"load_and_commit_config","routers":"vsrx-lab-02","router_count":1,"action":"commit","authorization":"allowed","result":"ok","duration_ms":3456,"error_kind":"","error":"","reason":"","metadata":"config_bytes=1234 dry_run=false"},"message":"audit"}
```

### Example: Failure

```json
{"timestamp":"2026-07-12T18:32:16.500Z","level":"INFO","target":"audit","fields":{"correlation_id":"req-1720805536500000000","caller":"devops","tool":"execute_junos_command","routers":"vsrx-lab-03","router_count":1,"action":"read","authorization":"allowed","result":"error","duration_ms":5001,"error_kind":"timeout","error":"NETCONF session timed out after 5000ms","reason":"","metadata":"format=text"},"message":"audit"}
```

### Example: Denial

```json
{"timestamp":"2026-07-12T18:32:17.250Z","level":"INFO","target":"audit","fields":{"correlation_id":"req-1720805537250000000","caller":"readonly-token","tool":"load_and_commit_config","routers":"vsrx-lab-01","router_count":1,"action":"commit","authorization":"denied","result":"denied","duration_ms":0,"error_kind":"","error":"","reason":"tool_scope","metadata":""},"message":"audit"}
```

## Configuration

Both binaries support identical audit configuration:

### `rust-junosmcp`

| Flag | Environment Variable | Default | Description |
|------|---------------------|---------|-------------|
| `--audit-format` | `JMCP_AUDIT_FORMAT` | `text` | Output format: `text` or `json`. |
| `--audit-log-file` | `JMCP_AUDIT_LOG_FILE` | (none) | Optional file path to append JSON events to (in addition to stderr). |

### `rust-srxmcp`

| Flag | Environment Variable | Default | Description |
|------|---------------------|---------|-------------|
| `--audit-format` | `JMCP_SRX_AUDIT_FORMAT` | `text` | Output format: `text` or `json`. |
| `--audit-log-file` | `JMCP_SRX_AUDIT_LOG_FILE` | (none) | Optional file path to append JSON events to (in addition to stderr). |

## Retention & Forwarding

### journald

When running under systemd, audit events written to stderr are captured by `journald`. Query with:

```bash
journalctl -u rust-junosmcp.service --output=json | jq -r 'select(.TARGET == "audit")'
```

### File sink

When `--audit-log-file` is set, JSON events are appended to the specified file. The file is **append-only** — the server never rotates or truncates it. Use `logrotate` or equivalent for retention management:

```
/var/log/jmcp/audit.jsonl {
    daily
    rotate 90
    compress
    delaycompress
    missingok
    notifempty
    copytruncate
}
```

### SIEM / forwarding

Ingest via:

- **Filebeat / Fluentd / Vector** — tail the JSON log file or `journalctl` output.
- **Syslog sink** — deferred (see below).

Filter on `target == "audit"` to separate audit events from operational logs.

## Deferred Items

The following capabilities are planned but not yet implemented:

1. **Syslog / journald native sink** — currently, the tracing JSON layer writes to stderr only. A future release may add a dedicated syslog or journald subscriber.
2. **Built-in log rotation** — the server does not manage file rotation; use external tooling (`logrotate`, `systemd-tmpfiles`).
3. **Per-field encryption** — sensitive metadata fields (e.g., partial config diffs, router IPs) are not currently encrypted. A future release may add per-field envelope encryption for at-rest protection.

## Security & Privacy

- **No secrets in audit logs** — credentials, private keys, and passwords are never logged. The `metadata` field is allowlisted per tool (e.g., `command_count`, `dry_run`, `config_bytes`) and excludes all secret material.
- **Error messages are bounded** — the `error` field is truncated at 512 characters to prevent unbounded log growth from pathological failures.
- **Caller attribution** — every event records the bearer-token name or `"stdio"`, enabling per-caller audit trails even when multiple tokens share the same scope.

## Example Queries

### All denied requests in the last hour

```bash
journalctl -u rust-junosmcp.service --since "1 hour ago" --output=json \
  | jq -r 'select(.TARGET == "audit") | select(.fields.result == "denied")'
```

### Top 10 slowest successful commands

```bash
jq -r 'select(.target == "audit") | select(.fields.result == "ok") | "\(.fields.duration_ms) \(.fields.tool) \(.fields.routers)"' \
  /var/log/jmcp/audit.jsonl \
  | sort -rn | head -10
```

### Failed commits by caller

```bash
jq -r 'select(.target == "audit") | select(.fields.action == "commit") | select(.fields.result == "error") | "\(.fields.caller) \(.fields.routers) \(.fields.error)"' \
  /var/log/jmcp/audit.jsonl
```
