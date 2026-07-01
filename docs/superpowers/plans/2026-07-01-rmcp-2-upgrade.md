# rmcp 0.8.5 → 2.0.0 Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `rmcp` from 0.8.5 to 2.0.0 in both binaries to clear RUSTSEC-2026-0189, preserving all behavior and keeping LAN bearer clients working via a configurable `Host` allowlist.

**Architecture:** rmcp 2.0.0 ships the DNS-rebinding fix as a default-DENY, loopback-only `Host` allowlist on the Streamable HTTP transport. Our used API surface (macros, `Parameters`, `ErrorData`, model types, `StreamableHttpService::new`, and the `http::request::Parts`→`Extensions` auth mechanism) is byte-compatible across the bump, so the only functional change is supplying an explicit allowlist, exposed as new `--allowed-host` / `--disable-host-check` CLI flags on both binaries.

**Tech Stack:** Rust, rmcp 2.0.0, axum 0.8, tower, clap 4, ureq (tests).

## Global Constraints

- Target `rmcp = "2"` (resolves 2.0.0) in BOTH `rust-junosmcp/Cargo.toml` and `rust-srxmcp/Cargo.toml`. Keep the exact 5 features: `server, macros, transport-io, schemars, transport-streamable-http-server`.
- Preserve ALL current behavior: tool surface, auth semantics, RFC 6750 401 bodies, blocklist/scope ordering. The `caller_ctx` bearer mechanism (`extensions.get::<http::request::Parts>()`) must keep working (rmcp 2.0.0 still inserts `Parts` into `Extensions` — verified).
- Host allowlist policy: **default = loopback only** (`localhost`, `127.0.0.1`, `::1` — rmcp's secure default). `--allowed-host <HOST>` (repeatable) EXTENDS the loopback defaults. `--disable-host-check` disables the check entirely and MUST log a `tracing::warn!`. If both are supplied, `--disable-host-check` wins.
- Acceptance gate: `cargo audit` reports **no RUSTSEC-2026-0189**; `cargo test --workspace` 0 failures; `cargo fmt -- --check` clean.
- Deploy LAN authority: junos `192.168.1.194:30031`, srx `192.168.1.194:30032` (container ct601 on pve2). The exact `--allowed-host` VALUE (host-only vs host:port) is determined by the matching rule confirmed in Task 1 Step 3.
- CLI field names (use verbatim): `allowed_host: Vec<String>` and `disable_host_check: bool`.

---

### Task 1: Bump rmcp to 2.0.0 — compile + existing tests green

**Files:**
- Modify: `rust-junosmcp/Cargo.toml:30`, `rust-srxmcp/Cargo.toml:29` (version `"0.8"` → `"2"`)
- Modify (only if compile/test forces it): `rust-junosmcp/src/http_transport.rs`, `rust-srxmcp/src/http_transport.rs`
- Touch: `Cargo.lock` (via `cargo update`)

**Interfaces:**
- Consumes: nothing.
- Produces: a workspace that builds and tests green against rmcp 2.0.0, with `http_transport::serve` still on `StreamableHttpServerConfig::default()` (loopback-only). Task 2 changes `serve`'s signature.

- [ ] **Step 1: Bump the dependency in both crates**

In `rust-junosmcp/Cargo.toml` and `rust-srxmcp/Cargo.toml`, change the rmcp line from `version = "0.8"` to `version = "2"`, leaving the `features = [...]` array exactly as-is:

```toml
rmcp = { version = "2", features = [
    "server",
    "macros",
    "transport-io",
    "schemars",
    "transport-streamable-http-server",
] }
```

- [ ] **Step 2: Update the lockfile and build**

Run: `cargo update -p rmcp && cargo build --workspace 2>&1 | tail -40`
Expected: `rmcp v2.0.0` resolves; the workspace compiles. Per the migration research our API surface is unchanged, so `server.rs` / model types should need NO edits. If a transitive `schemars`/`http`/`axum` major bump causes a compile error in `rust-junosmcp-core` arg structs, resolve it minimally (align the `schemars` derive version) and note it in the report. Do NOT refactor beyond what the compiler requires.

- [ ] **Step 3: Confirm the Host-matching rule in rmcp 2.0.0 source**

The registry now has the source. Read the Host-check logic to learn (a) whether the allowlist entry is matched against the `Host` header's **host portion** (port stripped) or the full `host:port` authority, and (b) the exact API to set the list (public `allowed_hosts` field vs a `with_allowed_hosts()` builder vs `disable_allowed_hosts()`):

Run: `ls ~/.cargo/registry/src/*/rmcp-2.0.0/src/transport/streamable_http_server/ && grep -rn "allowed_hosts\|fn with_allowed_hosts\|fn disable_allowed_hosts\|Host\b" ~/.cargo/registry/src/*/rmcp-2.0.0/src/transport/streamable_http_server/ | head -40`
Record in the report: the matching rule (host-only vs authority) and the setter API. This decides the `--allowed-host` value the deploy needs (Task 4) and the exact code in Task 2.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: 0 failures. The HTTP integration tests (`http_smoke`, `http_reload`) POST to `127.0.0.1:<port>`, so their `Host` is `127.0.0.1:<port>`, which the default loopback allowlist should accept.

**Contingency:** if `http_smoke`/`http_reload` now FAIL with HTTP 403, the default allowlist is matching the full `host:port` authority (so `127.0.0.1:<port>` ≠ `127.0.0.1`). Fix by making `serve` add the bind authority to the allowlist. In BOTH `http_transport.rs`, replace `StreamableHttpServerConfig::default()` with a config that also allows the bind `addr`. Using the setter confirmed in Step 3, e.g. if `allowed_hosts` is a public `Vec<String>`:

```rust
let mut http_cfg = StreamableHttpServerConfig::default();
http_cfg.allowed_hosts.push(addr.to_string());          // e.g. "127.0.0.1:34567"
http_cfg.allowed_hosts.push(addr.ip().to_string());     // e.g. "127.0.0.1"
let svc = StreamableHttpService::new(
    handler_factory,
    Arc::new(LocalSessionManager::default()),
    http_cfg,
);
```

(If the field is private, use the builder form `StreamableHttpServerConfig::default().with_allowed_hosts([...])` instead — pass the loopback names PLUS `addr.to_string()`.) Re-run `cargo test --workspace` to green. This code is superseded by Task 2's full wiring; the point of doing it here is only to keep the suite green.

- [ ] **Step 5: fmt + commit**

Run: `cargo fmt && cargo fmt -- --check`

```bash
git add rust-junosmcp/Cargo.toml rust-srxmcp/Cargo.toml Cargo.lock rust-junosmcp/src/http_transport.rs rust-srxmcp/src/http_transport.rs
git commit -m "build(deps): bump rmcp 0.8.5 -> 2.0.0 (#97)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019mPwHV2n6YmBTd5j8HcAAJ"
```

(If Step 4 needed no http_transport change, drop those two paths from `git add`.)

---

### Task 2: Configurable Host allowlist (`--allowed-host` / `--disable-host-check`)

**Files:**
- Modify: `rust-junosmcp/src/cli.rs:15-79` (add two fields to `Cli`)
- Modify: `rust-srxmcp/src/main.rs:24-52` (add two fields to `Cli`)
- Modify: `rust-junosmcp/src/http_transport.rs:23-37` (serve signature + config build)
- Modify: `rust-srxmcp/src/http_transport.rs:17-30` (serve signature + config build)
- Modify: `rust-junosmcp/src/main.rs:199-206` and `rust-srxmcp/src/main.rs:181` (pass new args)
- Test: `rust-junosmcp/tests/http_smoke.rs` (add host-allowlist tests)

**Interfaces:**
- Consumes: Task 1's rmcp-2.0.0 workspace; the Host-matching rule + setter API recorded in Task 1 Step 3.
- Produces: `serve(handler, addr, token_store, allowed_hosts: Vec<String>, disable_host_check: bool [, tls])` in both crates; CLI fields `allowed_host: Vec<String>`, `disable_host_check: bool`.

- [ ] **Step 1: Write the failing integration tests (junos)**

Append to `rust-junosmcp/tests/http_smoke.rs`. These reuse the existing harness but need a spawn variant that passes host flags and an `http_post` variant that overrides the `Host` header. Add these helpers and tests:

```rust
/// Spawn with extra CLI args appended (e.g. --allowed-host / --disable-host-check).
fn spawn_with_args(
    inv_path: &std::path::Path,
    tokens_path: &std::path::Path,
    extra: &[&str],
) -> Server {
    let port = pick_port();
    let mut argv = vec![
        "-f", inv_path.to_str().unwrap(),
        "-t", "streamable-http",
        "-H", "127.0.0.1",
        "-p", &port.to_string(),
        "--tokens-file", tokens_path.to_str().unwrap(),
    ];
    argv.extend_from_slice(extra);
    let mut child = Command::new(binary_path())
        .args(&argv)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    // readiness wait + stderr drain, identical to spawn()
    let stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut ready = false;
    loop {
        if Instant::now() > deadline { break; }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => { if line.contains("streamable-http listening") { ready = true; break; } }
            Err(_) => break,
        }
    }
    if !ready { let _ = child.kill(); panic!("server did not start within 15s"); }
    let drain = std::thread::spawn(move || {
        let mut sink = String::new();
        loop {
            sink.clear();
            match reader.read_line(&mut sink) { Ok(0) | Err(_) => break, Ok(_) => {} }
        }
    });
    Server { child, port, _stderr_drain: drain }
}

/// POST an `initialize` with an explicit Host header; return the HTTP status.
fn post_init_with_host(port: u16, host: &str) -> u16 {
    let req = ureq::post(&format!("http://127.0.0.1:{port}/mcp"))
        .set("Accept", "application/json, text/event-stream")
        .set("Host", host);
    match req.send_json(init_body()) {
        Ok(resp) => resp.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("transport error: {e}"),
    }
}

#[test]
fn disallowed_host_is_rejected_403() {
    ensure_built();
    let inv = write_inv(r#"{"r1":{"ip":"203.0.113.1","port":1,"username":"u","auth":{"type":"password","password":"x"}}}"#);
    let toks = write_tokens(r#"{"version":1,"tokens":[]}"#);
    // No --allowed-host: only loopback is allowed.
    let s = spawn(inv.path(), toks.path());
    let code = post_init_with_host(s.port, "evil.example.com");
    assert_eq!(code, 403, "a Host outside the allowlist must be rejected (DNS-rebinding guard)");
}

#[test]
fn allowed_host_flag_permits_custom_host() {
    ensure_built();
    let inv = write_inv(r#"{"r1":{"ip":"203.0.113.1","port":1,"username":"u","auth":{"type":"password","password":"x"}}}"#);
    let toks = write_tokens(r#"{"version":1,"tokens":[]}"#);
    // Allow a custom authority; then a request with that Host must pass the host gate.
    // NOTE: value form (host vs host:port) per the rule confirmed in Task 1 Step 3.
    let s = spawn_with_args(inv.path(), toks.path(), &["--allowed-host", "friendly.example.com"]);
    let code = post_init_with_host(s.port, "friendly.example.com");
    // Passes the Host gate → reaches auth, which returns 401 (no bearer). 401, NOT 403, proves the host was allowed.
    assert_eq!(code, 401, "an allowlisted Host must pass the Host gate (then 401 for missing bearer)");
}

#[test]
fn disable_host_check_allows_any_host() {
    ensure_built();
    let inv = write_inv(r#"{"r1":{"ip":"203.0.113.1","port":1,"username":"u","auth":{"type":"password","password":"x"}}}"#);
    let toks = write_tokens(r#"{"version":1,"tokens":[]}"#);
    let s = spawn_with_args(inv.path(), toks.path(), &["--disable-host-check"]);
    let code = post_init_with_host(s.port, "anything.example");
    // Host gate disabled → reaches auth → 401 for missing bearer (NOT 403).
    assert_eq!(code, 401, "--disable-host-check must bypass the Host gate");
}
```

Rationale for the 401-not-403 assertions: the auth middleware runs and rejects the missing bearer with 401 only if the request first passed rmcp's Host gate; a 403 means the Host gate rejected it. This cleanly distinguishes "host allowed" from "host denied" without a valid token.

- [ ] **Step 2: Run the new tests — verify they fail**

Run: `cargo test -p rust-junosmcp --test http_smoke allowed_host_flag_permits_custom_host disable_host_check_allows_any_host 2>&1 | tail -20`
Expected: FAIL to compile (unknown flags `--allowed-host` / `--disable-host-check` cause the child to exit non-zero → "server did not start"). `disallowed_host_is_rejected_403` may already pass on the default. The two flag tests are RED until Steps 3-5.

- [ ] **Step 3: Add the CLI flags (both binaries)**

In `rust-junosmcp/src/cli.rs`, inside `pub struct Cli`, add after the `ssh_accept_new_host_keys` field (line 78), before the closing brace:

```rust
    /// Additional Host authorities to accept on the streamable-http endpoint,
    /// beyond the loopback defaults (localhost, 127.0.0.1, ::1). Repeatable.
    /// Set this to the host/authority clients actually send (e.g. the LAN IP)
    /// or off-loopback clients are rejected with HTTP 403 (DNS-rebinding guard).
    #[arg(long)]
    pub allowed_host: Vec<String>,

    /// Disable the streamable-http Host allowlist entirely (accept any Host).
    /// Reintroduces the RUSTSEC-2026-0189 exposure; bearer auth still applies.
    /// Off by default.
    #[arg(long)]
    pub disable_host_check: bool,
```

In `rust-srxmcp/src/main.rs`, inside `struct Cli`, add after the `known_hosts_file` field (line 51), before the closing brace:

```rust
    /// Additional Host authorities to accept on the streamable-http endpoint,
    /// beyond the loopback defaults (localhost, 127.0.0.1, ::1). Repeatable.
    #[arg(long)]
    allowed_host: Vec<String>,

    /// Disable the streamable-http Host allowlist entirely (accept any Host).
    /// Reintroduces RUSTSEC-2026-0189 exposure; bearer auth still applies.
    #[arg(long)]
    disable_host_check: bool,
```

- [ ] **Step 4: Extend `serve` to build the allowlist (both binaries)**

In `rust-junosmcp/src/http_transport.rs`, change the `serve` signature and the config construction. New signature (add the two params BEFORE the `#[cfg(feature = "tls")]` tls param):

```rust
pub async fn serve(
    handler: JmcpHandler,
    addr: SocketAddr,
    token_store: Option<Arc<ArcSwap<TokenStore>>>,
    allowed_hosts: Vec<String>,
    disable_host_check: bool,
    #[cfg(feature = "tls")] tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<()> {
    let handler_factory = move || Ok::<_, std::io::Error>(handler.clone());

    let http_cfg = build_http_config(allowed_hosts, disable_host_check);
    let svc = StreamableHttpService::new(
        handler_factory,
        Arc::new(LocalSessionManager::default()),
        http_cfg,
    );
```

Add a helper in the same file (above `serve`). Use the setter API confirmed in Task 1 Step 3 — the version below assumes a public `allowed_hosts: Vec<String>` field and a `disable_allowed_hosts()` method (adjust to the builder form if the field is private):

```rust
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;

/// Build the streamable-http server config, applying the Host allowlist policy.
/// Default = rmcp's loopback-only allowlist (localhost/127.0.0.1/::1); each
/// `--allowed-host` value extends it. `--disable-host-check` turns the gate off.
fn build_http_config(allowed_hosts: Vec<String>, disable_host_check: bool) -> StreamableHttpServerConfig {
    if disable_host_check {
        tracing::warn!(
            "--disable-host-check: streamable-http Host allowlist DISABLED; accepting any Host header. \
             This reintroduces RUSTSEC-2026-0189 (DNS rebinding); bearer auth still applies."
        );
        return StreamableHttpServerConfig::default().disable_allowed_hosts();
    }
    let mut cfg = StreamableHttpServerConfig::default(); // loopback defaults
    cfg.allowed_hosts.extend(allowed_hosts);
    cfg
}
```

Do the identical change in `rust-srxmcp/src/http_transport.rs` (its `serve` has no `tls` param, so append the two params at the end):

```rust
pub async fn serve(
    handler: JmcpSrxHandler,
    addr: SocketAddr,
    token_store: Option<Arc<ArcSwap<TokenStore>>>,
    allowed_hosts: Vec<String>,
    disable_host_check: bool,
) -> Result<()> {
```

with the same `build_http_config` helper and `StreamableHttpServerConfig` construction. If the earlier Task 1 contingency added `addr`-derived hosts here, replace that with this helper (keep loopback defaults; the bind-authority hack is no longer needed because tests now spawn with explicit Host handling and the default already covers `127.0.0.1`).

- [ ] **Step 5: Pass the new args at both call sites**

In `rust-junosmcp/src/main.rs` at the `http_transport::serve(` call (line 199), insert the two args before the tls arg:

```rust
            http_transport::serve(
                handler,
                addr,
                token_store,
                args.allowed_host.clone(),
                args.disable_host_check,
                #[cfg(feature = "tls")]
                tls_cfg,
            )
            .await?;
```

In `rust-srxmcp/src/main.rs` (line 181):

```rust
    http_transport::serve(
        handler,
        addr,
        token_store,
        args.allowed_host.clone(),
        args.disable_host_check,
    )
    .await
```

- [ ] **Step 6: Build, run the new tests + full suite**

Run: `cargo fmt && cargo test -p rust-junosmcp --test http_smoke 2>&1 | tail -20`
Expected: all `http_smoke` tests PASS, including the three new ones (`disallowed_host_is_rejected_403`, `allowed_host_flag_permits_custom_host`, `disable_host_check_allows_any_host`).

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: 0 failures.

- [ ] **Step 7: Commit**

```bash
git add rust-junosmcp/src/cli.rs rust-srxmcp/src/main.rs rust-junosmcp/src/http_transport.rs rust-srxmcp/src/http_transport.rs rust-junosmcp/src/main.rs rust-junosmcp/tests/http_smoke.rs
git commit -m "feat(http): configurable Host allowlist for streamable-http (#97)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019mPwHV2n6YmBTd5j8HcAAJ"
```

---

### Task 3: Prose, docs, and audit-clean verification

**Files:**
- Modify: `rust-junosmcp-core/src/cancel.rs` (refresh "rmcp 0.8.5" prose)
- Modify: `README.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces: docs + a recorded `cargo audit` result showing RUSTSEC-2026-0189 gone.

- [ ] **Step 1: Refresh cancel.rs prose**

In `rust-junosmcp-core/src/cancel.rs`, the doc comments reference `rmcp 0.8.5` (lines ~4, 11, 15). Update the version references to `rmcp 2.0.0`. Do NOT change code — these are comments describing cancellation behavior. If the described behavior (rmcp detaching futures on client disconnect, issue #44) is unchanged in 2.0.0, keep the description and only bump the version number; if you cannot confirm, change "rmcp 0.8.5" → "rmcp 2.0.0" and leave the behavioral prose intact.

- [ ] **Step 2: Run cargo audit — the acceptance gate**

Run: `cargo audit 2>&1 | tail -30` (install if missing: `cargo install cargo-audit --locked`)
Expected: **no `RUSTSEC-2026-0189`**. Record the full output in the report. Any remaining advisories (`anyhow` RUSTSEC-2026-0190 unsound-warning, yanked `aes`) are out of scope for #97 — note them but they do not block. (If CI's `audit` job denies warnings via `RUSTFLAGS`/config, confirm whether those two are warnings-only or hard failures in `deny.toml`/CI; if they now hard-fail the job, report it — a decision for the controller, do not silently add ignores.)

- [ ] **Step 3: Update README + CHANGELOG**

In `README.md`, find where streamable-http / auth flags are documented and add the two new flags (`--allowed-host`, `--disable-host-check`) with a one-line security note: off-loopback clients must be allowlisted or they get 403. Match surrounding style.

In `CHANGELOG.md`, add under an Unreleased/next `### Security` (or `### Changed`) section:

```markdown
### Security
- Upgrade `rmcp` 0.8.5 → 2.0.0, closing RUSTSEC-2026-0189 (DNS rebinding in the
  Streamable HTTP transport). The transport now enforces a `Host` allowlist
  (default: loopback only). New flags `--allowed-host <HOST>` (repeatable) and
  `--disable-host-check` configure it; off-loopback deployments MUST pass
  `--allowed-host` for their LAN authority or clients receive HTTP 403.
```

- [ ] **Step 4: fmt + commit**

Run: `cargo fmt -- --check`

```bash
git add rust-junosmcp-core/src/cancel.rs README.md CHANGELOG.md
git commit -m "docs: document rmcp 2.0 Host allowlist flags + audit note (#97)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019mPwHV2n6YmBTd5j8HcAAJ"
```

---

### Task 4: Deploy to ct601 (pve2) + live smoke both endpoints

**Files:** none (operational). Also update the systemd unit files if they live in `packaging/` — check.

**Interfaces:**
- Consumes: the full branch (Tasks 1-3).

- [ ] **Step 1: Build release + stage to ct601**

Run: `cargo build --release -p rust-junosmcp -p rust-srxmcp`
Deploy path (pve2 hosts ct601): `scp` both binaries to `pve2.mechub.org:/tmp/`, then `ssh root@pve2.mechub.org` → `pct exec 601 -- systemctl stop <svc>` → `pct push 601 /tmp/<bin> /usr/local/bin/<bin>` → `chown root:root` → restart. Back up current binaries first (`.bak-$(date +%Y%m%d-%H%M%S)`, keep 2 most recent). See memory `rust_junosmcp_container_601.md` for the exact recipe (note: container is on **pve2**, not pve3).

- [ ] **Step 2: Add `--allowed-host` to both systemd units**

In ct601, edit the two unit files (`rust-junosmcp.service`, `rust-srxmcp.service`) `ExecStart` to append `--allowed-host 192.168.1.194` (or `192.168.1.194:30031` / `:30032` per the matching rule from Task 1 Step 3). `systemctl daemon-reload` + restart both. Confirm both units `active`.

- [ ] **Step 3: Verify tool surface + Host allowlist live (junos :30031)**

Drive the endpoint over JSON-RPC with the bearer from `~/.claude.json`:
- `tools/list` → expect 16 tools (incl. `commit_check_config`) — proves the upgraded binary serves the full surface.
- Raw `curl` with `Host: 192.168.1.194` (the allowlisted authority) → **200** on `initialize`.
- Raw `curl` with `Host: evil.example.com` → **403** (Host gate active).

- [ ] **Step 4: Verify srx endpoint (:30032)**

`tools/list` against `http://192.168.1.194:30032/mcp` → expect the srx tool set (10 tools). One read-only call (e.g. `srxmcp_status`) → success. `curl` with a bogus Host → 403.

- [ ] **Step 5: Record results** in the SDD ledger (tool counts, 200/403 outcomes, versions).

---

## Self-Review

**Spec coverage:**
- rmcp `"0.8"`→`"2"` both crates, features unchanged → Task 1. ✔
- API-compat (no server.rs changes) verified by compile+tests → Task 1. ✔
- `--allowed-host` / `--disable-host-check`, loopback default, extend semantics, disable+warn → Task 2. ✔
- Preserve auth mechanism → covered by existing http_smoke auth tests passing (Task 1/2). ✔
- 200/403/disable integration test → Task 2 Step 1. ✔
- `cargo audit` clean of RUSTSEC-2026-0189 → Task 3 Step 2. ✔
- cancel.rs prose → Task 3 Step 1. ✔
- README/CHANGELOG → Task 3 Step 3. ✔
- Deploy + `--allowed-host` in units + live smoke both endpoints → Task 4. ✔

**Placeholder scan:** No TBD/TODO. The one genuine build-time unknown (Host host-vs-authority matching + exact setter API) is resolved in Task 1 Step 3 with a concrete investigation command, and both Task 1 Step 4 (contingency) and Task 2 Step 4 give exact code for the likely-public-field case with an explicit builder fallback. The 200/403 tests pin the behavior regardless.

**Type consistency:** `serve(...)` gains `allowed_hosts: Vec<String>, disable_host_check: bool` in both crates (Task 2 Step 4), fed by CLI fields `allowed_host: Vec<String>` / `disable_host_check: bool` (Task 2 Step 3) via `args.allowed_host.clone()` / `args.disable_host_check` (Task 2 Step 5). `build_http_config(Vec<String>, bool) -> StreamableHttpServerConfig` named identically in both http_transport.rs. Consistent.

**Risk note for implementer:** the highest-impact failure is a wrong `--allowed-host` value (host vs host:port) silently 403-ing production. Task 1 Step 3 MUST nail the matching rule, and Task 4 Step 3's live 200/403 curl is the final proof before declaring done.
