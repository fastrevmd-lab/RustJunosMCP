# Remote Transport + Auth — Design

**Date:** 2026-05-05
**Status:** Approved (brainstorming complete; pre-implementation plan)
**Sub-project of:** v0.2 (sub-project #2 of 4)

## Context

`v0.1` shipped a stdio-only MCP server. `v0.2`'s sub-project #1 (blocklist
guardrails) shipped on 2026-05-05 (PR #3). This sub-project lights up the
`streamable-http` transport that has been forward-compat plumbed through the
CLI since v0.1, plus bearer-token authentication and per-token authorization
scopes.

The Python reference [Juniper/junos-mcp-server] ships an HTTP transport with
a plaintext `.tokens` file (one token per line) and an optional separate
`token-manager` binary. We deliberately diverge from that wire format
(operator chose "inspired but cleaner" during brainstorming) while keeping
the deployment story simple.

[Juniper/junos-mcp-server]: https://github.com/Juniper/junos-mcp-server

## Goals

- Real `--transport streamable-http` support (currently `bail!`'d at
  startup).
- Bearer-token authentication with hashed tokens at rest.
- Per-token authorization scopes covering routers and tools.
- Optional native TLS via `rustls`; default deployment posture is plain HTTP
  on loopback behind a reverse proxy.
- `SIGHUP`-driven hot reload of the tokens file (so credential rotation does
  not require a restart and does not interrupt in-flight calls).
- Token store management as a `rust-junosmcp token …` subcommand of the same
  binary — no separate `token-manager` binary.
- Fail-closed defaults: refuse to start `streamable-http` without auth;
  refuse to bind off-loopback over plain HTTP. Both refusals have explicit
  named escape-hatch flags.
- Backward compatible with v0.1 stdio: no behavioral change on `-t stdio`,
  no schema changes to `devices.json`.
- Pure, exhaustively unit-tested token store with no I/O dependencies.

## Non-goals (deferred)

Each is a candidate for a future sub-project or a v0.3+ task:

- **Glob scopes** — `routers: ["lab-*"]` parses today as a literal name (no
  match). The schema cleanly extends; we do not enable globbing in this
  sub-project.
- **Per-tool finer-grained scopes** beyond the v0.1 tool set (e.g. read-only
  vs. read-write within `execute_junos_command`).
- **Out-of-band token issuance** (an HTTP endpoint that mints tokens). All
  minting is via the local `token` subcommand.
- **Filesystem-watch auto-reload** of the tokens file. `SIGHUP` only.
- **Audit log of allowed calls.** Tracing logs cover this.
- **Rate limiting**, **CORS**, **OIDC / OAuth2 / mTLS** — out of scope.
- **Drop-in compat with the Python repo's `.tokens` plaintext format.**
  Operators migrating from the Python server re-mint via `token add`.
- **Hot reload of `devices.json` / blocklist** — folded into v0.2's future
  `reload_devices` sub-project.
- **Streamable-HTTP session affinity** beyond what rmcp 0.8 provides
  out-of-the-box.

## Architecture

A new workspace member, `rust-junosmcp-auth/`, holds the pure auth logic.
The shape mirrors the blocklist `Policy`: a typed in-memory store with no
I/O dependencies, plus a thin file loader.

```
.
├── rust-junosmcp-core/        (unchanged)
├── rust-junosmcp-auth/        NEW
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs             (public surface)
│       ├── token.rs           (Secret, TokenHash, mint, verify)
│       ├── store.rs           (TokenStore, ScopeSet, lookup)
│       └── file.rs            (TokenStoreFile, load/save, atomic write)
├── rust-junosmcp/             (binary; gains http + token subcommand wiring)
│   └── src/
│       ├── auth_layer.rs      NEW (tower middleware)
│       ├── http_transport.rs  NEW (axum router + rmcp mount)
│       ├── tls.rs             NEW (rustls config builder, behind `tls` feat)
│       ├── token_cmd.rs       NEW (clap subcommand impl)
│       ├── caller.rs          NEW (CallerCtx struct)
│       ├── cli.rs             (subcommand wired in; new flags)
│       ├── server.rs          (handler accepts Option<Arc<ArcSwap<TokenStore>>>)
│       └── main.rs            (transport branch, signal handler)
```

`rust-junosmcp-core` does not learn about auth. The handler in
`rust-junosmcp/src/server.rs` carries the optional token store and the
adapter functions check scopes before calling into core's tool handlers.
This preserves the layering established in sub-project #1 (auth and
authorization in the binary, blocklist in core), and keeps `core` free of
the rand / sha2 / axum dependency tree.

### `CallerCtx` and `ScopeSet`

```rust
// rust-junosmcp/src/caller.rs
pub struct CallerCtx {
    pub token_name: String,
    pub routers: ScopeSet,
    pub tools: ScopeSet,
}

// rust-junosmcp-auth/src/store.rs
pub enum ScopeSet {
    Wildcard,            // matches any name (i.e. raw input was ["*"])
    Allowlist(Vec<String>),  // matches only listed names; empty = matches nothing
}

impl ScopeSet {
    pub fn allows(&self, name: &str) -> bool { /* ... */ }
}
```

The two-variant enum makes `["*"]` and `["specific", "names"]` distinct at
the type level (no special-case strings inside an `Allowlist`), and an
empty `Allowlist` cleanly represents "scoped to nothing" without
collision with `Wildcard`.

## Schema (`tokens.json`)

The token file is a **separate file** from `devices.json`. Default path is
not assumed; the operator passes `--tokens-file` explicitly.

```json
{
  "version": 1,
  "tokens": [
    {
      "name": "claude-desktop-prod",
      "hash": "sha256:VYV9w8c...lz8",
      "routers": ["*"],
      "tools": ["execute_junos_command", "get_junos_config"],
      "created_at": "2026-05-05T18:00:00Z"
    },
    {
      "name": "ci-readonly",
      "hash": "sha256:RkR7K1x...qP4",
      "routers": ["lab-r1", "lab-r2"],
      "tools": [
        "get_router_list",
        "gather_device_facts",
        "get_junos_config",
        "junos_config_diff"
      ],
      "created_at": "2026-05-05T18:01:00Z"
    }
  ]
}
```

### Field rules

- **`version`** — currently `1`. Mismatched version is a fatal load error
  with a message naming the supported version.
- **`name`** — operator-facing identifier; must be unique across the file
  (case-sensitive). Used in tracing logs and `token list` output.
- **`hash`** — the literal string `"sha256:" + base64url_unpadded(SHA-256(secret))`,
  where `secret` is the base64url-unpadded ASCII string the operator
  received at mint time. Verification rehashes the candidate string and
  compares with `subtle::ConstantTimeEq`.
- **`routers`** — list of inventory keys (literal names from `devices.json`),
  or `["*"]` for all. Empty list is permitted; the entry will never match
  a router-bound call (it can still call router-less tools per the
  semantics below). Loading emits a `WARN` for empty `routers`.
- **`tools`** — list of tool names from the v0.1 set, or `["*"]` for all.
  Empty list lints as above.
- **`created_at`** — RFC 3339 timestamp; informational, surfaced by
  `token list`. Set automatically on `add` / `rotate`.

### Validation

Fatal at load:
- duplicate `name`
- malformed `hash` (missing `"sha256:"` prefix; base64url-unpadded length
  ≠ 43; non-base64url chars)
- unknown tool name in `tools` (typos must not silently grant nothing)
- `version` not equal to `1`

Warning at load (entry kept):
- `routers` containing names absent from the current `devices.json`
- empty `routers` or empty `tools`

### Scope semantics

A call is **allowed** by the auth layer if and only if both:

- `tool_name ∈ token.tools` OR `token.tools = ["*"]`, AND
- the tool either takes no `router_name` argument, OR
  `router_name ∈ token.routers` OR `token.routers = ["*"]`.

Special case for `get_router_list` (the only v0.1 tool with no
`router_name` argument that returns router data): the response list is
**filtered** to routers in `token.routers` before being returned. An
empty-scope token gets an empty list, not a 401/403 and not the full
inventory.

Globs (`?`, `*`-as-wildcard, `lab-*`) are NOT honored in `routers` /
`tools` in this sub-project. The exact string `"*"` is the only wildcard.

## CLI surface

### Server flags (additions)

```
rust-junosmcp \
  -f devices.json \
  -t streamable-http \
  -H 127.0.0.1 -p 30030 \
  --tokens-file /etc/jmcp/tokens.json \
  [--tls-cert /etc/jmcp/server.crt --tls-key /etc/jmcp/server.key] \
  [--allow-no-auth] \
  [--allow-insecure-bind]
```

- `-H` / `-p` — already accepted in v0.1, now actually used for binding.
- `--tokens-file <path>` — required when `-t streamable-http` unless
  `--allow-no-auth`. On `-t stdio`, presence of this flag emits `WARN` and
  is otherwise ignored.
- `--tls-cert` and `--tls-key` — both must be set together. Enables rustls
  termination. Without them, the server speaks plain HTTP.
- `--allow-no-auth` — opt out of bearer auth. Refuses to start unless the
  bind host is `127.0.0.1` or `::1`. Logs a startup `WARN`.
- `--allow-insecure-bind` — required to bind off-loopback over plain HTTP.
  Not required when TLS is configured.

### Refusal matrix

| Transport | tokens-file | Host | TLS | Outcome |
|---|---|---|---|---|
| `stdio` | (any) | n/a | n/a | OK (`--tokens-file` warns if set) |
| `streamable-http` | yes | any | any (off-loopback needs TLS or `--allow-insecure-bind`) | OK |
| `streamable-http` | no, no `--allow-no-auth` | any | any | refuse at startup |
| `streamable-http` | no, `--allow-no-auth` | loopback | any | OK with WARN |
| `streamable-http` | no, `--allow-no-auth` | non-loopback | any | refuse at startup |
| `streamable-http` | yes | non-loopback | plain | refuse unless `--allow-insecure-bind` |
| `streamable-http` | yes | non-loopback | TLS | OK |

### `token` subcommand

```
rust-junosmcp token add \
  --tokens-file /etc/jmcp/tokens.json \
  --name claude-desktop-prod \
  --routers '*' \
  --tools execute_junos_command,get_junos_config \
  [--server-pid <pid>]

rust-junosmcp token list   --tokens-file /etc/jmcp/tokens.json
rust-junosmcp token revoke --tokens-file /etc/jmcp/tokens.json --name claude-desktop-prod \
  [--server-pid <pid>]
rust-junosmcp token rotate --tokens-file /etc/jmcp/tokens.json --name claude-desktop-prod \
  [--server-pid <pid>]
```

Behavior:

- `add` — generates 32 random bytes from `OsRng`, base64url-encodes
  unpadded (`~43` ASCII chars), prints the secret to **stdout** (and only
  stdout) once, writes `sha256:<digest>` to the file. Fails if
  `--name` already exists. `--routers` / `--tools` accept comma-separated
  lists or the literal `*`.
- `list` — prints `name | routers | tools | created_at`. Never prints
  hashes. Never prints secrets (it doesn't have them — only hashes are
  stored).
- `revoke` — removes the entry. Idempotent: missing name → exit 0 with
  `INFO`.
- `rotate` — `revoke` + `add` under the same scopes. Prints the new secret
  to stdout. Fails if name does not exist.
- `--server-pid <pid>` — optional convenience; subcommand sends `SIGHUP`
  to the named pid after a successful write. Errors from `kill(2)` are
  reported as a `WARN` but do not fail the subcommand (the file write
  already succeeded).

All file-mutating subcommands use atomic write: write to
`tokens.json.<random>.tmp` in the same directory, `fsync`, then `rename`
over the original. Exit codes are non-zero on any validation or I/O
failure.

## Request flow

```
HTTP request (POST /mcp)
  ├─ TLS termination (rustls; only if --tls-cert/--tls-key supplied)
  ├─ axum outer router
  │   └─ AuthLayer (tower middleware)
  │       ├─ load Arc<TokenStore> via arc_swap.load()
  │       ├─ extract Authorization: Bearer <secret>
  │       │   ├─ missing / wrong scheme              → 401 + WWW-Authenticate
  │       │   ├─ no matching token                    → 401, log auth_failed
  │       │   └─ matched TokenEntry e                  → request.extensions.insert(CallerCtx::from(e))
  │       └─ pass through
  ├─ rmcp streamable-http handler decodes JSON-RPC
  ├─ #[tool] adapter pulls CallerCtx from request extensions
  │   ├─ tool ∉ ctx.tools                              → JmcpError::ToolNotInScope
  │   ├─ tool takes router_name AND
  │   │   router_name ∉ ctx.routers                    → JmcpError::RouterNotInScope
  │   ├─ tool == get_router_list                       → filter result by ctx.routers
  │   └─ otherwise                                      → continue
  ├─ existing blocklist Policy check (unchanged)
  └─ tool body runs against rustEZ Device
```

### Order of checks

`auth → tool scope → router scope → blocklist → execute`. The blocklist
runs after authz so denials still log a `token_name`.

### Stdio path

Unchanged. No middleware, no `CallerCtx`, no scope checks. The handler's
optional `Arc<ArcSwap<TokenStore>>` is `None` and adapters short-circuit
the auth/scope checks before they look at the request.

## Error surface

Scope denials (`ToolNotInScope` / `RouterNotInScope`) originate in the
binary's `#[tool]` adapter — they are not reachable from
`rust-junosmcp-core`'s tool handlers, which never see a `CallerCtx`. They
therefore live in `rust-junosmcp`, not `core`. The adapter converts them
to MCP `CallToolResult { isError: true }` via the same `to_call_result`
helper used for `JmcpError::Denied`.

`AuthRequired` / `AuthInvalid` are HTTP-level concerns; they live in the
auth middleware and never reach the tool layer.

`TokenStoreInvalid` is a load-time fatal in `rust-junosmcp-auth`; it
surfaces from `TokenStoreFile::load` and is reported by the binary at
startup or on `SIGHUP`-failed-reload.

```rust
// rust-junosmcp/src/server.rs (binary-local)
#[derive(Debug, thiserror::Error)]
enum ScopeError {
    #[error("token '{token}' is not authorized for tool '{tool}'")]
    ToolNotInScope { token: String, tool: &'static str },

    #[error("token '{token}' is not authorized for router '{router}' (tool '{tool}')")]
    RouterNotInScope { token: String, router: String, tool: &'static str },
}

// rust-junosmcp/src/auth_layer.rs (HTTP-only)
enum AuthError {
    Required,         // 401 + WWW-Authenticate: Bearer
    Invalid,          // 401, no header echo
    StoreUnavailable, // 503 (only during startup races; should not happen post-init)
}

// rust-junosmcp-auth/src/file.rs
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("token store invalid: {0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
```

`ToolNotInScope` and `RouterNotInScope` mirror `JmcpError::Denied` in
their textual shape so an LLM client sees a consistent denial style
across blocklist and authz refusals.

## Tracing

Every authenticated request emits one `INFO` log on success and one
`WARN` on any denial, with these fields:

- `token_name` (always set after AuthLayer runs)
- `tool` (set after rmcp dispatch)
- `router_name` (Some / None depending on the tool)
- `outcome` ∈ {`allowed`, `auth_failed`, `tool_not_in_scope`,
  `router_not_in_scope`, `blocklist_denied`, `error`}
- `remote_addr` (for streamable-http only; from axum's
  `ConnectInfo<SocketAddr>`)

**Token secrets** and **token hashes** are never logged. Auth failures log
`token_name = "<unknown>"`.

## Hot reload (SIGHUP)

A tokio signal handler is registered in `main` only when running
`-t streamable-http` with a `--tokens-file`. On `SIGHUP`:

1. Reload the file via `TokenStoreFile::load`.
2. If the new file validates cleanly, `arc_swap.store(Arc::new(new_store))`
   and `INFO` log with new token count.
3. If validation fails, keep the old store and `ERROR` log with the
   reason. The server keeps running with the previous tokens.

In-flight requests use whichever `Arc<TokenStore>` snapshot they captured
at AuthLayer entry. There is no torn-read window and no per-request `Arc`
clone of the store body.

## TLS

Optional. When `--tls-cert` and `--tls-key` are both supplied, the binary
loads a `rustls::ServerConfig` (PEM via `rustls-pemfile`) and wraps the
listener with `tokio-rustls`. Cipher suites and protocol versions follow
rustls defaults (TLS 1.2 + 1.3, no SSLv3 / TLS 1.0 / TLS 1.1). The
`rustls` deps are gated behind a Cargo `tls` feature so stdio-only builds
(e.g. CI without `aws-lc-rs`) do not pull rustls.

The `tls` feature is **on** by default; users who explicitly want a
slimmer binary can `cargo build --no-default-features` to drop it.

Reverse-proxy deployments (the documented default) ignore all of this and
let nginx / caddy / traefik / Cloudflare terminate TLS upstream of the
loopback listener.

## rmcp 0.8 dependency verification (planning-time spike)

T0 of the implementation plan will be a 30-minute spike to confirm rmcp
0.8 streamable-http exposes:

1. A way to mount the rmcp service onto an outer axum router (so we can
   stack tower middleware in front of the JSON-RPC handler).
2. A way for `#[tool]` methods to read request extensions (so the adapter
   can pull `CallerCtx` set by the middleware).

Two known-good fallbacks if either piece is awkward in 0.8:

- **A.** Hand-roll an outer axum router; mount the rmcp axum
  sub-application beneath. AuthLayer stuffs `CallerCtx` into a
  request-scoped extension that the rmcp handler can read via the request
  extensions map.
- **B.** If `#[tool]` macros hide per-request extensions entirely, fall
  back to a `RequestId` → `CallerCtx` keyed `dashmap::DashMap` populated
  by the middleware on entry and drained by the adapter on exit. Less
  elegant; same observable behavior.

Whichever fallback (if any) the spike picks, it does not change the
schema, the CLI surface, the request flow's logical order, or the
testing strategy. Only the wiring inside `JmcpHandler` is affected.

## New dependencies

`[workspace.dependencies]` additions:

- `axum = "0.8"` — http framework; `rust-junosmcp` only.
- `tower = "0.5"`, `tower-http = "0.6"` — middleware; `rust-junosmcp` only.
- `arc-swap = "1"` — `rust-junosmcp-auth` and `rust-junosmcp`.
- `sha2 = "0.10"` — `rust-junosmcp-auth` only.
- `rand = "0.8"` — `rust-junosmcp-auth` only.
- `subtle = "2"` — constant-time hash compare; `rust-junosmcp-auth` only.
- `base64ct = { version = "1", features = ["alloc"] }` — base64url
  encode/decode; `rust-junosmcp-auth` only.
- `chrono = { version = "0.4", default-features = false, features = ["serde", "clock"] }`
  — RFC 3339 timestamps; `rust-junosmcp-auth` only.
- `rustls = "0.23"`, `tokio-rustls = "0.26"`, `rustls-pemfile = "2"` —
  feature-gated; `rust-junosmcp` only.

Existing rmcp dep gains the streamable-http transport feature (exact name
TBD by the spike — likely `transport-streamable-http-axum`).

## Testing

### Unit (`rust-junosmcp-auth`)

- Mint round-trip: secret → hash → lookup hits; rotated secret no longer
  hits.
- Wrong secret misses; correct hash but constant-time compare exercised.
- Duplicate `name` on `add` returns a typed error.
- Revoke is idempotent (missing → no-op).
- Scope match: literal router/tool, `["*"]`, `[]` (empty), unknown router.
- File load:
  - missing version
  - version != 1
  - duplicate name
  - malformed hash (no prefix, wrong length, non-base64url chars)
  - unknown tool name
  - unknown router name (warn-only)
  - empty file (no `tokens` key)
- Atomic write semantics: target file unchanged on simulated mid-write
  failure (test by writing a marker file, then triggering a write that
  panics between `fsync` and `rename`).

### CLI integration (`rust-junosmcp/tests/token_subcommand.rs`)

- `token add` writes an entry, prints a non-empty 43-char secret to
  stdout, the on-disk hash is not equal to the secret.
- `token list` after add includes the name, never the hash, never the
  secret.
- `token rotate` produces a new secret and a different hash with the same
  scopes.
- `--server-pid` sends SIGHUP to a sleep-loop dummy process (assert via
  the dummy process exiting on receipt of `SIGHUP`).

### Streamable-HTTP smoke (`rust-junosmcp/tests/http_smoke.rs`)

Spawns the binary on `127.0.0.1:0` (ephemeral port) with `--tokens-file`
populated, sends real HTTP, asserts:

1. `tools/list` with no `Authorization` header → HTTP 401.
2. `tools/list` with wrong bearer → HTTP 401.
3. `tools/list` with a token that has `tools: ["get_router_list"]` returns
   an MCP success but only the routers in the token's `routers` scope.
4. `tools/call` for `execute_junos_command` with a token scoped to `r1`
   but call asks for `r2` → `CallToolResult { isError: true }` containing
   "not authorized for router".
5. `tools/call` with matching scopes that hits a blocklist deny rule →
   `CallToolResult { isError: true }` containing "denied by blocklist"
   (proves order: auth → scope → blocklist).
6. `SIGHUP` after revoking a token → next call with the revoked token
   returns 401 within 200ms.

### TLS smoke (`rust-junosmcp/tests/tls_smoke.rs`)

Self-signed cert in `tempdir`, boot with `--tls-cert / --tls-key`, hit
the listener with a `rustls`-aware client. Single happy-path test;
mismatched-key / missing-key fatal-startup cases are covered by CLI
integration tests.

### Stdio smoke (existing)

Unchanged. Both existing tests must still pass — proof that the stdio
path is undisturbed by this work.

## Backward compatibility

- Existing `devices.json`: untouched.
- v0.1 stdio deployments: identical CLI flags work, identical behavior.
  The new `--tokens-file` and `--allow-*` flags exist but are no-ops on
  stdio (`--tokens-file` emits a `WARN` if set without
  `-t streamable-http`).
- The blocklist Policy interaction is unchanged — auth + scope is a layer
  in front of, not a replacement for, the blocklist.
- `rust-junosmcp-core` does not gain an auth dependency.
- The `tls` Cargo feature is on by default; users who need a slimmer
  binary can opt out via `cargo build --no-default-features`.

## Sub-project boundaries

This is sub-project #2 of v0.2. Sub-projects #3 (PFE + batch) and #4
(templates + inventory mutation) build on top of, but do not require,
this work. Specifically:

- The future `add_device` / `reload_devices` tools (sub-project #4) will
  share the `SIGHUP`-style reload pattern but reload `devices.json` /
  blocklist, not the tokens file. They MAY choose to send their own
  signal or share `SIGHUP`; that decision lives in sub-project #4.
- The future PFE / batch tools (sub-project #3) will plug into the same
  blocklist + scope check chain. New tool names will need to land in the
  `tokens.json` schema validator's known-tools list; this is a one-line
  change per new tool.

## Open issues for the planning spike

1. Confirm the rmcp 0.8 feature flag name for streamable-http
   (`transport-streamable-http-axum` is the working assumption).
2. Confirm rmcp 0.8 exposes request extensions readable from `#[tool]`
   methods. If not, fall back path B from §rmcp dependency verification.
3. Confirm rustls 0.23 + tokio-rustls 0.26 binds cleanly under axum 0.8
   (this is well-trodden ground; flagging only because the dep matrix
   moves frequently).
4. Pin a `chrono` version that compiles under our existing
   `default-features = false` MSRV (workspace currently does not use
   `chrono`; if there's an `OffsetDateTime` we'd reuse, prefer it).
