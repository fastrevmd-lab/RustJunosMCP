# Caller-Attributed Audit Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every tool call on both `rust-junosmcp` and `rust-srxmcp` a uniform, caller-attributed audit event (success/failure/denied/unsettled), with redaction, JSON/file sink options, and tests.

**Architecture:** A new `rust-junosmcp-audit` crate provides an RAII `AuditScope` guard (generalizing the existing `UpgradeAuditGuard`) that emits one `target="audit"` event per call on `Drop`, plus a configurable `init_tracing`. Both binaries construct an `AuditScope` at the top of every `#[tool]` handler and set its outcome; denial paths set `deny(reason)`.

**Tech Stack:** Rust 2021, tracing / tracing-subscriber 0.3 (features `env-filter`, `json`), rmcp 2.0, sha2, serde_json.

## Global Constraints

- **Shared crate versions:** `tracing = "0.1"`, `tracing-subscriber = "0.3"` (must enable features `["env-filter", "json"]`), `sha2 = "0.10"` (workspace), `serde_json = "1"`.
- **Audit event target is the string `"audit"`** (`tracing::info!(target: "audit", …)`), level INFO.
- **Uniform field names on every event:** `correlation_id`, `caller`, `tool`, `routers`, `router_count`, `action`, `authorization` (`allowed`|`denied`|`no_auth`), `result` (`ok`|`error`|`denied`|`unsettled`), `duration_ms`; on error also `error` (Display, truncated to 512 bytes) and `error_kind`.
- **`caller`** = `CallerCtx.token_name`, or `"stdio"` when ctx is `None`. `authorization` = `no_auth` when caller is `"stdio"`, `denied` when the outcome is Denied, else `allowed`.
- **Redaction by construction:** only the per-tool safe metadata in the design's allowlist may be attached. NEVER attach config bodies, rendered templates, template vars, command output, or credentials.
- **`0`/unset disables** the optional audit file. Format default = `text`.
- **Parity:** identical schema and behavior on both binaries.
- **Do not remove** the existing per-phase SRX workflow audit events; only align their field names/target (Task 4).
- Doc comments on public items. Commit after each task. Branch `feat/132-caller-attributed-audit` (already checked out).

---

## File Structure

**New crate `rust-junosmcp-audit/`:**
- `Cargo.toml`
- `src/lib.rs` — module wiring + re-exports.
- `src/schema.rs` — `AuditOutcome`, `AuditValue`, field-name consts.
- `src/scope.rs` — `AuditScope` (RAII guard, `new`/`meta`/`succeed`/`fail`/`fail_kind`/`deny`, `Drop`).
- `src/init.rs` — `AuditFormat`, `AuditConfig`, `init_tracing`, the file `MakeWriter`.
- `src/testutil.rs` — `CapturingWriter` + `run_with_capture` (behind `cfg(any(test, feature = "test-util"))`).

**Modified:**
- `Cargo.toml` (workspace) — add member + `dashmap`-style entry; ensure `tracing-subscriber` workspace dep enables `json` (add feature at crate level).
- `rust-junosmcp/Cargo.toml`, `rust-srxmcp/Cargo.toml` — add `rust-junosmcp-audit` dep (+ `test-util` dev-dep).
- `rust-junosmcp/src/main.rs`, `rust-srxmcp/src/main.rs` — build `AuditConfig`, call `rust_junosmcp_audit::init_tracing`.
- `rust-junosmcp/src/cli.rs`, `rust-srxmcp/src/cli.rs` — 2 flags each.
- `rust-junosmcp/src/server.rs` — `AuditScope` in all 17 handlers; remove 4 inline `"audit"` blocks + `UpgradeAuditGuard` reconciliation.
- `rust-srxmcp/src/server.rs` — `AuditScope` in all 9 handlers.
- `rust-srxmcp-core/src/workflows/idp_package.rs` — align phase-audit field names/target.
- `docs/AUDIT.md` (new), `README.md`.

**New tests:**
- `rust-junosmcp-audit/src/*` inline `#[cfg(test)]`.
- `rust-junosmcp/tests/audit.rs`, `rust-srxmcp/tests/audit.rs` — field + redaction + denial assertions.

---

### Task 1: `rust-junosmcp-audit` crate — schema + `AuditScope` guard

**Files:**
- Create: `rust-junosmcp-audit/Cargo.toml`, `src/lib.rs`, `src/schema.rs`, `src/scope.rs`, `src/testutil.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: `AuditScope`, `AuditOutcome`, `AuditValue`; `run_with_capture`/`CapturingWriter` (test-util).

- [ ] **Step 1: Workspace member + crate manifest**

Add `"rust-junosmcp-audit"` to workspace `members` in `Cargo.toml`.

`rust-junosmcp-audit/Cargo.toml`:
```toml
[package]
name        = "rust-junosmcp-audit"
version     = "0.1.0"
edition.workspace     = true
license.workspace     = true
repository.workspace  = true
authors.workspace     = true
description = "Caller-attributed audit events and configurable audit sink for rust-junosmcp / rust-srxmcp."

[features]
test-util = []

[dependencies]
rust-junosmcp-auth = { path = "../rust-junosmcp-auth" }
tracing            = { workspace = true }
tracing-subscriber = { version = "0.3", features = ["env-filter", "json", "fmt", "registry"] }
serde_json         = { workspace = true }

[dev-dependencies]
tracing            = { workspace = true }
```

- [ ] **Step 2: Write `schema.rs`**

```rust
//! Audit event vocabulary: outcomes, safe metadata values, field-name constants.

use std::fmt::Display;

/// Terminal outcome of an audited tool call.
#[derive(Debug, Clone)]
pub enum AuditOutcome {
    /// Handler completed successfully.
    Succeeded,
    /// Handler returned an error. `kind` is a stable category; `msg` is a
    /// bounded, non-secret Display of the error.
    Failed { kind: &'static str, msg: String },
    /// Authorization denied the call before work began.
    Denied { reason: &'static str },
    /// Guard dropped without an outcome set (client cancel / disconnect).
    Unsettled,
}

/// A safe, non-secret metadata value.
#[derive(Debug, Clone)]
pub enum AuditValue {
    Str(String),
    U64(u64),
    Bool(bool),
}

impl From<&str> for AuditValue { fn from(v: &str) -> Self { AuditValue::Str(v.to_string()) } }
impl From<String> for AuditValue { fn from(v: String) -> Self { AuditValue::Str(v) } }
impl From<u64> for AuditValue { fn from(v: u64) -> Self { AuditValue::U64(v) } }
impl From<usize> for AuditValue { fn from(v: usize) -> Self { AuditValue::U64(v as u64) } }
impl From<bool> for AuditValue { fn from(v: bool) -> Self { AuditValue::Bool(v) } }

impl Display for AuditValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditValue::Str(s) => write!(f, "{s}"),
            AuditValue::U64(n) => write!(f, "{n}"),
            AuditValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// Truncate an error Display to a bounded length for the audit event.
pub fn bounded_error(e: impl Display) -> String {
    let s = e.to_string();
    if s.len() <= 512 { s } else { format!("{}…", &s[..512]) }
}
```

- [ ] **Step 3: Write `testutil.rs`**

```rust
//! Tracing-capture helper for asserting on `audit`-target output in tests.
#![cfg(any(test, feature = "test-util"))]

use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// A cloneable in-memory writer collecting everything written to it.
#[derive(Clone, Default)]
pub struct CapturingWriter(pub Arc<Mutex<Vec<u8>>>);

impl Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl<'a> MakeWriter<'a> for CapturingWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer { self.clone() }
}

/// Run `f` with a temporary subscriber capturing INFO output; return the text.
pub fn run_with_capture<F: FnOnce()>(f: F) -> String {
    let cap = CapturingWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(cap.clone())
        .with_ansi(false)
        .with_target(true)
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = cap.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}
```

- [ ] **Step 4: Write the failing `scope.rs` tests**

Create `scope.rs` with the impl (Step 5) plus:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::run_with_capture;
    use rust_junosmcp_auth::caller::CallerCtx;
    use rust_junosmcp_auth::ScopeSet;

    fn ctx(name: &str) -> CallerCtx {
        CallerCtx { token_name: name.into(), routers: ScopeSet::Wildcard, tools: ScopeSet::Wildcard }
    }

    #[test]
    fn success_emits_ok_with_duration_and_meta() {
        let out = run_with_capture(|| {
            let mut a = AuditScope::new(Some(&ctx("ci")), "load_and_commit_config", "commit", vec!["r1".into()]);
            a.meta("config_bytes", 1234u64);
            a.succeed();
        });
        assert!(out.contains("audit"));
        assert!(out.contains("tool=\"load_and_commit_config\""));
        assert!(out.contains("caller=\"ci\""));
        assert!(out.contains("authorization=\"allowed\""));
        assert!(out.contains("result=\"ok\""));
        assert!(out.contains("config_bytes=1234"));
        assert!(out.contains("duration_ms="));
    }

    #[test]
    fn unsettled_when_dropped_without_outcome() {
        let out = run_with_capture(|| {
            let _a = AuditScope::new(Some(&ctx("ci")), "upgrade_junos", "upgrade", vec!["r1".into()]);
        });
        assert!(out.contains("result=\"unsettled\""));
    }

    #[test]
    fn deny_emits_denied_authorization() {
        let out = run_with_capture(|| {
            let mut a = AuditScope::new(Some(&ctx("ci")), "add_device", "add-device", vec![]);
            a.deny("tool_scope");
        });
        assert!(out.contains("authorization=\"denied\""));
        assert!(out.contains("result=\"denied\""));
        assert!(out.contains("reason=\"tool_scope\""));
    }

    #[test]
    fn stdio_caller_is_no_auth() {
        let out = run_with_capture(|| {
            let mut a = AuditScope::new(None, "get_router_list", "read", vec![]);
            a.succeed();
        });
        assert!(out.contains("caller=\"stdio\""));
        assert!(out.contains("authorization=\"no_auth\""));
    }
}
```

> `ScopeSet::Wildcard` and `CallerCtx` field names verified in
> `rust-junosmcp-auth/src/caller.rs` + `scope.rs`. If the `ScopeSet` re-export
> path differs, adjust the `use`.

- [ ] **Step 5: Write the `AuditScope` implementation (top of `scope.rs`)**

```rust
//! RAII audit guard: emits exactly one `target="audit"` event on Drop.

use crate::schema::{bounded_error, AuditOutcome, AuditValue};
use rust_junosmcp_auth::caller::CallerCtx;
use std::fmt::Display;
use std::time::Instant;

fn mint_correlation_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("req-{nanos}")
}

/// One audited tool call. Construct at the top of a handler, set an outcome,
/// and let it drop — the drop emits the audit event.
pub struct AuditScope {
    correlation_id: String,
    caller: String,
    tool: &'static str,
    routers: Vec<String>,
    action: &'static str,
    started: Instant,
    outcome: AuditOutcome,
    metadata: Vec<(&'static str, AuditValue)>,
}

impl AuditScope {
    /// Build for a call. `caller` is the token name, or `"stdio"` when absent.
    pub fn new(
        ctx: Option<&CallerCtx>,
        tool: &'static str,
        action: &'static str,
        routers: Vec<String>,
    ) -> Self {
        Self {
            correlation_id: mint_correlation_id(),
            caller: ctx.map(|c| c.token_name.clone()).unwrap_or_else(|| "stdio".into()),
            tool,
            routers,
            action,
            started: Instant::now(),
            outcome: AuditOutcome::Unsettled,
            metadata: Vec::new(),
        }
    }

    /// Attach a safe metadata field (never secrets).
    pub fn meta(&mut self, key: &'static str, val: impl Into<AuditValue>) {
        self.metadata.push((key, val.into()));
    }

    /// Mark success.
    pub fn succeed(&mut self) { self.outcome = AuditOutcome::Succeeded; }

    /// Mark failure with a generic kind (`"error"`).
    pub fn fail(&mut self, error: impl Display) {
        self.outcome = AuditOutcome::Failed { kind: "error", msg: bounded_error(error) };
    }

    /// Mark failure with a specific stable kind (e.g. `"timeout"`, `"lease_busy"`).
    pub fn fail_kind(&mut self, kind: &'static str, error: impl Display) {
        self.outcome = AuditOutcome::Failed { kind, msg: bounded_error(error) };
    }

    /// Mark an authorization denial with a reason.
    pub fn deny(&mut self, reason: &'static str) {
        self.outcome = AuditOutcome::Denied { reason };
    }
}

impl Drop for AuditScope {
    fn drop(&mut self) {
        let duration_ms = self.started.elapsed().as_millis() as u64;
        let routers = self.routers.join(",");
        let router_count = self.routers.len() as u64;
        // Flatten safe metadata into a single string field to keep the macro
        // call fixed-arity; JSON consumers still get `metadata` as key=val pairs.
        let metadata = self
            .metadata
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");

        let authorization = match &self.outcome {
            AuditOutcome::Denied { .. } => "denied",
            _ if self.caller == "stdio" => "no_auth",
            _ => "allowed",
        };
        let (result, error_kind, error, reason) = match &self.outcome {
            AuditOutcome::Succeeded => ("ok", "", String::new(), ""),
            AuditOutcome::Failed { kind, msg } => ("error", *kind, msg.clone(), ""),
            AuditOutcome::Denied { reason } => ("denied", "", String::new(), *reason),
            AuditOutcome::Unsettled => ("unsettled", "", String::new(), ""),
        };

        tracing::info!(
            target: "audit",
            correlation_id = %self.correlation_id,
            caller = %self.caller,
            tool = %self.tool,
            routers = %routers,
            router_count = router_count,
            action = %self.action,
            authorization = %authorization,
            result = %result,
            duration_ms = duration_ms,
            error_kind = %error_kind,
            error = %error,
            reason = %reason,
            metadata = %metadata,
            "audit"
        );
    }
}
```

> Design choice recorded: the event uses a **fixed-arity** macro call (empty
> `error`/`reason`/`metadata` strings when N/A) rather than conditional macro
> variants — keeps every event shape identical for SIEM parsing.

- [ ] **Step 6: `lib.rs`**

```rust
//! Caller-attributed audit events for rust-junosmcp / rust-srxmcp.

mod schema;
mod scope;
pub mod testutil;

pub use schema::{AuditOutcome, AuditValue};
pub use scope::AuditScope;
// `init` module added in Task 2.
```

- [ ] **Step 7: Run the tests**

Run: `cargo test -p rust-junosmcp-audit --features test-util`
Expected: PASS (4 scope tests).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock rust-junosmcp-audit/
git commit -m "feat(132): rust-junosmcp-audit crate with AuditScope guard"
```

---

### Task 2: Configurable sink — `AuditConfig` + `init_tracing`

**Files:**
- Create: `rust-junosmcp-audit/src/init.rs`
- Modify: `rust-junosmcp-audit/src/lib.rs`

**Interfaces:**
- Produces: `AuditFormat`, `AuditConfig`, `init_tracing(&AuditConfig)`.

- [ ] **Step 1: Write the failing init test**

Add to `init.rs` (below the impl):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_written_to_audit_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        // Build only the file layer + a temporary subscriber (not the global one,
        // which other tests may have set). Verify a target="audit" event lands as JSON.
        let handle = FileHandle::open(&path).unwrap();
        let layer = audit_file_layer(handle.clone());
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "audit", tool = "t", result = "ok", "audit");
            tracing::info!(target: "not_audit", "ignored");
        });
        drop(handle); // flush
        let body = std::fs::read_to_string(&path).unwrap();
        let line = body.lines().next().expect("one audit line");
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["fields"]["tool"], "t");
        assert!(!body.contains("ignored"), "non-audit events must not hit the audit file");
    }
}
```

> Add `tempfile = "3"` to `rust-junosmcp-audit` `[dev-dependencies]`.

- [ ] **Step 2: Write `init.rs`**

```rust
//! Configurable tracing/audit sink: stderr (text or JSON) plus an optional
//! dedicated JSON audit file. Replaces the binaries' previous `init_tracing`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// stderr output format for logs and audit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFormat { Text, Json }

impl AuditFormat {
    /// Parse from a CLI/env string; unknown → Text.
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("json") { AuditFormat::Json } else { AuditFormat::Text }
    }
}

/// Audit / logging configuration.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    pub format: AuditFormat,
    /// When set, `target="audit"` events are also appended as JSON lines here.
    pub audit_log_file: Option<PathBuf>,
}

/// A cloneable append writer over a shared file handle.
#[derive(Clone)]
pub struct FileHandle(Arc<Mutex<File>>);

impl FileHandle {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(FileHandle(Arc::new(Mutex::new(f))))
    }
}

impl Write for FileHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().write(buf) }
    fn flush(&mut self) -> std::io::Result<()> { self.0.lock().unwrap().flush() }
}

impl<'a> MakeWriter<'a> for FileHandle {
    type Writer = FileHandle;
    fn make_writer(&'a self) -> Self::Writer { self.clone() }
}

/// A JSON fmt layer filtered to `target == "audit"`, writing to `handle`.
pub fn audit_file_layer<S>(handle: FileHandle) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .json()
        .with_writer(handle)
        .with_filter(filter_fn(|meta| meta.target() == "audit"))
}

/// Initialize the global subscriber. Idempotent (`try_init`).
pub fn init_tracing(cfg: &AuditConfig) {
    let env = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let stderr = match cfg.format {
        AuditFormat::Text => stderr.boxed(),
        AuditFormat::Json => tracing_subscriber::fmt::layer().json().with_writer(std::io::stderr).boxed(),
    };
    let file_layer = cfg
        .audit_log_file
        .as_ref()
        .and_then(|p| FileHandle::open(p).ok())
        .map(audit_file_layer);

    let _ = tracing_subscriber::registry()
        .with(env)
        .with(stderr)
        .with(file_layer) // Option<Layer> is itself a Layer (no-op when None)
        .try_init();
}
```

- [ ] **Step 3: Export from `lib.rs`**

```rust
mod init;
pub use init::{AuditConfig, AuditFormat};
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p rust-junosmcp-audit --features test-util`
Run: `cargo clippy -p rust-junosmcp-audit --all-targets --all-features -- -D warnings`
Expected: PASS; clean. (Resolve any `Layer` trait-bound issues against the pinned tracing-subscriber.)

- [ ] **Step 5: Commit**

```bash
git add rust-junosmcp-audit/
git commit -m "feat(132): configurable audit sink (JSON format + optional audit file)"
```

---

### Task 3: Integrate audit into `rust-junosmcp` (all 17 handlers) + CLI + tests

**Files:**
- Modify: `rust-junosmcp/Cargo.toml`, `src/cli.rs`, `src/main.rs`, `src/server.rs`
- Create: `rust-junosmcp/tests/audit.rs`

**Interfaces:**
- Consumes: `rust_junosmcp_audit::{AuditScope, AuditConfig, AuditFormat, init_tracing}`.

- [ ] **Step 1: Dependency + CLI flags**

`rust-junosmcp/Cargo.toml`: add `rust-junosmcp-audit = { path = "../rust-junosmcp-audit" }` to `[dependencies]` and `rust-junosmcp-audit = { path = "../rust-junosmcp-audit", features = ["test-util"] }` to `[dev-dependencies]`.

`rust-junosmcp/src/cli.rs`, append to `Cli`:
```rust
    /// Audit/log output format for stderr: text or json.
    #[arg(long, env = "JMCP_AUDIT_FORMAT", default_value = "text")]
    pub audit_format: String,

    /// Optional file to append JSON audit lines to (in addition to stderr).
    #[arg(long, env = "JMCP_AUDIT_LOG_FILE")]
    pub audit_log_file: Option<std::path::PathBuf>,
```

- [ ] **Step 2: Init wiring in `main.rs`**

Replace the `bootstrap::init_tracing();` call (rust-junosmcp/src/main.rs:20) with:
```rust
    let audit_cfg = rust_junosmcp_audit::AuditConfig {
        format: rust_junosmcp_audit::AuditFormat::parse(&args.audit_format),
        audit_log_file: args.audit_log_file.clone(),
    };
    rust_junosmcp_audit::init_tracing(&audit_cfg);
```
(`args` is available after `Cli::parse()`; if init currently runs before parse, move it to just after parse. Confirm ordering by reading main.rs.)

- [ ] **Step 3: Add `AuditScope` to every handler — the recipe**

For EACH `#[tool]` handler in `rust-junosmcp/src/server.rs`, apply this transformation. The pattern (worked example: `execute_junos_command`, currently server.rs:337-352):

```rust
async fn execute_junos_command(
    &self,
    Parameters(args): Parameters<ExecuteCommandArgs>,
    extensions: Extensions,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let ctx = caller_ctx(&extensions);
    let mut audit = AuditScope::new(ctx, "execute_junos_command", "execute", vec![args.router_name.clone()]);

    if let Err(e) = self.check_tool_scope(ctx, "execute_junos_command") {
        audit.deny("tool_scope");
        return Self::scope_to_call_result(e);
    }
    if let Err(e) = self.check_router_scope(ctx, "execute_junos_command", &args.router_name) {
        audit.deny("router_scope");
        return Self::scope_to_call_result(e);
    }
    audit.meta("command", args.command.clone());

    let result = execute_command::handle(args, self.dm.clone(), self.policy.load_full()).await;
    match &result {
        Ok(v) => { audit.meta("output_bytes", v.to_string().len() as u64); audit.succeed(); }
        Err(e) => audit.fail(e),
    }
    Self::to_call_result(result)
}
```

Rules:
1. Build `audit` immediately after `let ctx = caller_ctx(&extensions);`, with `(tool_name, action, routers)` from the per-tool table below.
2. On each `check_tool_scope` failure → `audit.deny("tool_scope")` before the existing return. On `check_router_scope` failure → `audit.deny("router_scope")`.
3. Bind the core call to `let result = …;`, match `&result` to `succeed()` (attaching result-derived metadata) or `fail(e)`, then pass `result` to `to_call_result`.
4. Attach only the safe metadata named in the table.

Per-tool table (tool → action → routers expr → metadata):

| Handler | action | routers | arg metadata | result metadata |
|---------|--------|---------|--------------|-----------------|
| `get_router_list` | read | `vec![]` | — | `count` (names.len) |
| `gather_device_facts` | read | `vec![args.router_name.clone()]` | — | `output_bytes` |
| `execute_junos_command` | execute | `vec![args.router_name.clone()]` | `command` | `output_bytes` |
| `get_junos_config` | read | `vec![args.router_name.clone()]` | — | `output_bytes` |
| `junos_config_diff` | read | `vec![args.router_name.clone()]` | — | `output_bytes` |
| `load_and_commit_config` | commit | `vec![args.router_name.clone()]` | `config_bytes`(args.config.len), `config_sha256`(sha256 hex of args.config), `commit_confirmed`(if arg exists else omit), `comment_present`(args.comment.is_some) | — |
| `commit_check_config` | commit-check | `vec![args.router_name.clone()]` | `config_bytes`, `config_sha256` | — |
| `discard_candidate` | discard | `vec![args.router_name.clone()]` | — | — |
| `execute_junos_pfe_command` | execute | `vec![args.router_name.clone()]` | `command` | `output_bytes` |
| `execute_junos_command_batch` | execute-batch | `args.router_names.clone()` (see arg struct) | `command_count` | — |
| `render_and_apply_j2_template` | apply | router(s) from args | `template_name` (if present), `var_count`, `committed`(bool arg) | `rendered_bytes` (if surfaced) |
| `add_device` | add-device | `vec![]` | `name`, `host`, `auth_kind` | — |
| `reload_devices` | reload-inventory | `vec![]` | — | `device_count` (if surfaced) |
| `transfer_file` | transfer | `vec![args.router_name.clone()]` | `basename` | `sha256` (from result) |
| `fetch_file` | fetch | `vec![args.router_name.clone()]` | `basename` | `sha256` (from result) |
| `upgrade_junos` | upgrade | `vec![args.router_name.clone()]` | `basename`, `target_version` | — |
| `list_staged_files` | read | router(s) or `vec![]` | — | `count` (if surfaced) |

> Read each handler's arg struct in `rust-junosmcp-core/src/tools/*` to confirm
> exact field names (`router_name` vs `router_names`, `config`, `comment`,
> `template`, etc.). If a metadata source field does not exist on the arg/result,
> OMIT that metadata key — never invent or guess a value, and never attach a
> secret. Compute `config_sha256` with `sha2::Sha256` over the config bytes,
> hex-encoded.

- [ ] **Step 4: Remove the old inline audit + reconcile `UpgradeAuditGuard`**

Delete the 4 inline `tracing::info!(… "audit")` success/error blocks in
`transfer_file`, `fetch_file`, `upgrade_junos`, `list_staged_files` (their fields
are now `AuditScope` metadata). For `upgrade_junos`: keep `UpgradeAuditGuard` ONLY
if it still adds the cancel/disconnect signal beyond `AuditScope`'s own
`unsettled`-on-drop — since `AuditScope` now emits `unsettled` on drop, **remove
`UpgradeAuditGuard`** and its `UpgradeOutcome` enum, letting `AuditScope` cover all
paths uniformly. Verify no other references remain (`grep UpgradeAuditGuard`).

- [ ] **Step 5: Write the audit integration tests**

Create `rust-junosmcp/tests/audit.rs`. These are UNIT-style tests exercising the
handler audit paths via captured tracing where possible; for handlers that need a
live device, assert only the denial/redaction paths that do not reach the device.
Concretely, test the pure-guard behavior end to end and redaction:

```rust
//! Audit field + redaction assertions for rust-junosmcp.
use rust_junosmcp_audit::{testutil::run_with_capture, AuditScope};

#[test]
fn add_device_audit_omits_credentials() {
    // Simulate the metadata the handler attaches — must never include secrets.
    let out = run_with_capture(|| {
        let mut a = AuditScope::new(None, "add_device", "add-device", vec![]);
        a.meta("name", "r99");
        a.meta("host", "192.0.2.10");
        a.meta("auth_kind", "password");
        // NOTE: the handler must NOT attach the password; assert it never appears.
        a.succeed();
    });
    assert!(out.contains("auth_kind=password"));
    assert!(!out.contains("hunter2"));
}

#[test]
fn commit_audit_logs_hash_not_body() {
    let out = run_with_capture(|| {
        let mut a = AuditScope::new(None, "load_and_commit_config", "commit", vec!["r1".into()]);
        a.meta("config_bytes", 42u64);
        a.meta("config_sha256", "abc123");
        a.succeed();
    });
    assert!(out.contains("config_sha256=abc123"));
    assert!(!out.contains("pre-shared-key"));
}
```

> These guard-level redaction tests pin the contract that secrets are never
> attached. The reviewer will additionally verify (by reading the diff) that each
> handler attaches ONLY table-listed metadata — that source-level check is the
> real redaction guarantee; the tests pin the guard behavior.

- [ ] **Step 6: Build, test, clippy**

Run: `cargo build -p rust-junosmcp`
Run: `cargo test -p rust-junosmcp`
Run: `cargo clippy -p rust-junosmcp --all-targets -- -D warnings`
Expected: PASS; existing `upgrade_audit_guard_tests` removed/updated to match.

- [ ] **Step 7: Commit**

```bash
git add rust-junosmcp/
git commit -m "feat(132): caller-attributed audit across all rust-junosmcp tools"
```

---

### Task 4: Integrate audit into `rust-srxmcp` (all 9 handlers) + align workflow audit

**Files:**
- Modify: `rust-srxmcp/Cargo.toml`, `src/cli.rs`, `src/main.rs`, `src/server.rs`, `rust-srxmcp-core/src/workflows/idp_package.rs`
- Create: `rust-srxmcp/tests/audit.rs`

- [ ] **Step 1: Dep + CLI flags + init** (mirror Task 3 steps 1-2, `JMCP_SRX_AUDIT_FORMAT` / `JMCP_SRX_AUDIT_LOG_FILE`)

- [ ] **Step 2: `AuditScope` in each srx handler using `authorize_call`**

SRX uses `authorize_call(&extensions, tool, router)` which returns `Result<Option<&CallerCtx>, ScopeError>`. Because `authorize_call` bundles tool+router checks and can return `MissingCallerContext`, build the audit scope from `caller_ctx(&extensions)` FIRST, then map an authz error to a reason:

```rust
async fn get_chassis_cluster_status(
    &self,
    Parameters(args): Parameters<rust_srxmcp_core::ClusterStatusArgs>,
    extensions: Extensions,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let ctx = caller_ctx(&extensions);
    let mut audit = AuditScope::new(ctx, "get_chassis_cluster_status", "read", vec![args.router.clone()]);
    if let Err(e) = self.authorize_call(&extensions, "get_chassis_cluster_status", Some(&args.router)) {
        audit.deny(match e {
            ScopeError::MissingCallerContext => "missing_caller_context",
            ScopeError::RouterNotInScope { .. } => "router_scope",
            ScopeError::ToolNotInScope { .. } => "tool_scope",
        });
        return Self::scope_to_call_result(e);
    }
    let result = /* existing body → Result<Value/typed, _> */;
    match &result { Ok(_) => audit.succeed(), Err(e) => audit.fail(e) }
    /* existing conversion */
}
```

Apply to all 9 tools with this action/routers/metadata table:

| Handler | action | routers | metadata |
|---------|--------|---------|----------|
| `srxmcp_status` | read | `vec![]` | — |
| `get_chassis_cluster_status` | read | `vec![args.router.clone()]` | `output_bytes` |
| `get_srx_security_services_status` | read | `vec![args.router.clone()]` | `output_bytes` |
| `check_srx_feature_license` | read | `vec![args.router.clone()]` | `feature`(if arg) |
| `vpn_lifecycle_report` | read | `vec![args.router.clone()]` | `output_bytes` |
| `manage_idp_security_package` | idp-package | `vec![args.router.clone()]` | `action`(the sub-action), `target_version`(if surfaced) |
| `manage_appid_signature_package` | appid-package | `vec![args.router.clone()]` | `action`(sub-action) |
| `validate_chassis_cluster_health` | read | `vec![args.router.clone()]` | `output_bytes` |
| `collect_jtac_support_bundle` | collect | `vec![args.router.clone()]` | `bundle_bytes`(if surfaced) |

> The two `manage_*` tools already emit rich per-phase workflow audit. The
> top-level `AuditScope` adds one uniform success/failure/duration event; keep
> both. Confirm each handler's arg field is `router` (not `router_name`) by
> reading the arg structs in `rust-srxmcp-core`.

- [ ] **Step 3: Align the workflow phase-audit field names**

In `rust-srxmcp-core/src/workflows/idp_package.rs` `audit_phase_with_action`
(1466-1515): keep the per-phase events but ensure they use `target: "audit"` (they
do) and rename `request_id` → `correlation_id` and `caller` stays `caller`, so the
phase events share the top-level event's key vocabulary. Do not change phase
semantics. Update any test asserting the old `request_id` field name.

- [ ] **Step 4: Tests** (mirror Task 3 step 5): `rust-srxmcp/tests/audit.rs` with a denial-reason test (incl. `missing_caller_context`) and a redaction assertion.

- [ ] **Step 5: Build, test, clippy**

Run: `cargo build -p rust-srxmcp`
Run: `cargo test -p rust-srxmcp -p rust-srxmcp-core`
Run: `cargo clippy -p rust-srxmcp -p rust-srxmcp-core --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rust-srxmcp/ rust-srxmcp-core/
git commit -m "feat(132): caller-attributed audit across all rust-srxmcp tools"
```

---

### Task 5: Documentation + full-workspace verification

**Files:**
- Create: `docs/AUDIT.md`
- Modify: `README.md`

- [ ] **Step 1: `docs/AUDIT.md`**

Document, for SIEM/log consumers: the `target=audit` convention; the full field
table (`correlation_id`, `caller`, `tool`, `routers`, `router_count`, `action`,
`authorization`, `result`, `duration_ms`, `error_kind`, `error`, `reason`,
`metadata`); the `authorization` and `result` value enums; a JSON example line for a
success, a failure, and a denial; the four denial `reason` values (`tool_scope`,
`router_scope`, `inventory_readonly`, `missing_caller_context`); the
`--audit-format` / `--audit-log-file` flags (both binaries, both env prefixes); and
retention/forwarding guidance (journald + logrotate; the JSON file is append-only).
Note the deferred items (syslog sink, rotation tooling, encryption).

- [ ] **Step 2: README pointer**

Add a short "Audit logging" subsection to `README.md` linking `docs/AUDIT.md` and
listing the two flags with defaults.

- [ ] **Step 3: Full-workspace verification**

Run: `cargo test --workspace --all-targets --locked`
Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Run: `cargo fmt --check`
Run: `cargo audit`
Expected: all PASS / clean.

- [ ] **Step 4: Commit**

```bash
git add docs/AUDIT.md README.md
git commit -m "docs(132): document the audit event schema and sink config"
```

- [ ] **Step 5: (post-PR) Comment on #132** listing the deferred items (syslog/journald sink, rotation/retention tooling, per-field encryption).

---

## Self-Review

**Spec coverage:**
- Shared schema → Task 1 (`schema.rs`, `AuditScope`). ✅
- All outcomes (ok/error/denied/unsettled) → Task 1 `AuditOutcome` + Drop. ✅
- Every tool covered → Task 3 (17 junos) + Task 4 (9 srx). ✅
- Four denial points → Task 3 (tool/router scope) + Task 4 (`missing_caller_context`); inventory-readonly is surfaced as a `JmcpError` from the core `add_device`/`reload_devices` handlers, so it is captured by `audit.fail(e)` with the readonly error — **⚠ verify** it is distinguishable (see risk below). ✅/⚠
- Correlation/caller/tool/routers/action/authz/result/duration/metadata → Task 1 event. ✅
- Redaction → allowlist metadata (Task 3/4) + tests (Task 3/4). ✅
- Configurable sink → Task 2 (`init_tracing`, JSON + file). ✅
- Captured-tracing tests → Task 1 + Task 3/4. ✅
- Schema doc → Task 5. ✅

**Placeholder scan:** The per-tool tables intentionally say "if surfaced / if present" for metadata whose source field must be confirmed against the arg/result structs — these are bounded "read the struct, else omit" instructions, not open TODOs. No "handle appropriately" placeholders.

**Type consistency:** `AuditScope::new(Option<&CallerCtx>, &'static str, &'static str, Vec<String>)` used identically in both binaries. Field names in the Drop event match `docs/AUDIT.md` (Task 5) and the tests (Task 1/3/4). `AuditFormat::parse` used in both `main.rs` builders.

**Open risk carried into implementation (inventory-readonly denial):** the
inventory-readonly refusal is a `JmcpError::InventoryReadonly` returned by the core
handler, so with the recipe it lands as `result=error` (via `audit.fail`), not
`result=denied`. To classify it as a denial, Task 3's `add_device`/`reload_devices`
handlers should check for that specific error and call `audit.deny("inventory_readonly")`
instead of `fail`. Implementer: match `JmcpError::InventoryReadonly` explicitly in
those two handlers and use `deny`; all other errors use `fail`. Documented here so it
is not missed.
