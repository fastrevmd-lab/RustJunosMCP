# Per-Router In-Flight Limits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce an enabled-by-default limit of four concurrent streamable-HTTP requests per exact router name on both MCP endpoints, returning the existing stable 503 overload response when saturated.

**Architecture:** Extend `rust-junosmcp-limits` so the shared HTTP middleware buffers and replays size-limited `tools/call` bodies, extracts top-level router targets, and non-blockingly acquires one weak-registry semaphore permit per unique router. Router permits join the existing global/token permits in `GuardedBody`, while destructive handlers continue to acquire their separate cross-process device lease after HTTP admission.

**Tech Stack:** Rust 1.97 (pinned toolchain), Axum 0.8 middleware and bodies, Tokio semaphores, `serde_json`, rmcp 2 streamable HTTP, Clap 4, Cargo workspace tests.

## Global Constraints

- Work only in `/home/mharman/Projects/RustJunosMCP/.worktrees/issue-147-per-router-limits` on branch `agent/issue-147-per-router-limits`.
- Preserve exact, case-sensitive router names; do not normalize, trim, lowercase, or resolve inventory aliases in the limiter.
- `max_inflight_requests_per_router` defaults to `4`; `0` means unlimited.
- Overload is immediate HTTP 503 with `Retry-After: 1` and body `{"error":"overloaded","limit":"router_concurrency"}`; do not add a queue.
- Extract only top-level `router`, `router_name`, `routers`, and `router_names` values from `params.arguments` on `tools/call` requests.
- A multi-router request holds one permit per unique router until response end-of-stream; partial acquisition must roll back immediately.
- HTTP router admission happens before the existing `DeviceLeaseManager` acquisition; core and SRX workflows must not reacquire HTTP permits.
- Do not change MCP schemas, tool annotations, auth scopes, audit fields, device operations, lease timing, or stdio behavior.
- Add no new external runtime package: use the workspace's existing `serde_json`; `rust-junosmcp-core` and `tempfile` are test-only.
- Never hand-edit `Cargo.lock`; let Cargo update direct-dependency metadata.
- Do not run real-device or ignored integration tests without `CONFIRM_LAB_INTEGRATION=yes` and explicit target review.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `rust-junosmcp-limits/src/config.rs` | Shared default and startup logging for the new cap. |
| `rust-junosmcp-limits/src/router.rs` | Pure JSON-RPC router-target extraction and weak semaphore registry. |
| `rust-junosmcp-limits/src/concurrency.rs` | Buffer/replay requests, acquire router permits, preserve overload/body-limit behavior, and attach permits to responses. |
| `rust-junosmcp-limits/src/lib.rs` | Register the private router module; public API remains unchanged. |
| `rust-junosmcp-limits/Cargo.toml` | Existing workspace JSON dependency plus test-only lease dependencies. |
| `rust-junosmcp/src/cli.rs`, `rust-srxmcp/src/cli.rs` | Endpoint-specific flag/env definitions and parser tests. |
| `rust-junosmcp/src/main.rs`, `rust-srxmcp/src/main.rs` | Thread parsed values into the shared `LimitsConfig`. |
| `README.md`, `CHANGELOG.md`, `rust-srxmcp/CHANGELOG.md` | User-facing defaults, semantics, lease interaction, and release notes. |

---

### Task 1: Shared Configuration and Endpoint Wiring

**Files:**
- Modify: `rust-junosmcp-limits/src/config.rs:8-59`
- Modify: `rust-junosmcp/src/cli.rs:97-123,197-260`
- Modify: `rust-srxmcp/src/cli.rs:72-106,128-147`
- Modify: `rust-junosmcp/src/main.rs:229-236`
- Modify: `rust-srxmcp/src/main.rs:192-199`

**Interfaces:**
- Consumes: Existing `LimitsConfig`, Clap `Cli`, and the two explicit config literals in `main.rs`.
- Produces: `LimitsConfig::max_inflight_requests_per_router: usize`, `Cli::max_inflight_requests_per_router: usize` on both binaries, Junos env `JMCP_MAX_INFLIGHT_REQUESTS_PER_ROUTER`, and SRX env `JMCP_SRX_MAX_INFLIGHT_REQUESTS_PER_ROUTER`.

- [ ] **Step 1: Write failing shared-config and CLI tests**

In `rust-junosmcp-limits/src/config.rs`, extend `defaults_are_generous_and_enabled`:

```rust
assert_eq!(c.max_inflight_requests_per_token, 16);
assert_eq!(c.max_inflight_requests_per_router, 4);
```

In `rust-junosmcp/src/cli.rs`, add:

```rust
#[test]
fn per_router_limit_defaults_and_parses() {
    let default_cli = Cli::parse_from(["rust-junosmcp"]);
    assert_eq!(default_cli.max_inflight_requests_per_router, 4);

    let disabled = Cli::parse_from([
        "rust-junosmcp",
        "--max-inflight-requests-per-router",
        "0",
    ]);
    assert_eq!(disabled.max_inflight_requests_per_router, 0);

    let custom = Cli::parse_from([
        "rust-junosmcp",
        "--max-inflight-requests-per-router",
        "7",
    ]);
    assert_eq!(custom.max_inflight_requests_per_router, 7);
}
```

In `rust-srxmcp/src/cli.rs`, add:

```rust
#[test]
fn per_router_limit_defaults_and_parses() {
    let default_cli = Cli::parse_from(["rust-srxmcp"]);
    assert_eq!(default_cli.max_inflight_requests_per_router, 4);

    let disabled = Cli::parse_from([
        "rust-srxmcp",
        "--max-inflight-requests-per-router",
        "0",
    ]);
    assert_eq!(disabled.max_inflight_requests_per_router, 0);

    let custom = Cli::parse_from([
        "rust-srxmcp",
        "--max-inflight-requests-per-router",
        "7",
    ]);
    assert_eq!(custom.max_inflight_requests_per_router, 7);
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test -p rust-junosmcp-limits config::tests::defaults_are_generous_and_enabled
cargo test -p rust-junosmcp cli::tests::per_router_limit_defaults_and_parses
cargo test -p rust-srxmcp cli::tests::per_router_limit_defaults_and_parses
```

Expected: compilation fails because `max_inflight_requests_per_router` does not yet exist on `LimitsConfig` or either `Cli`.

- [ ] **Step 3: Add the shared config field, default, and startup log field**

In `LimitsConfig`, insert after `max_inflight_requests_per_token`:

```rust
/// Max concurrent in-flight requests per target router. `0` disables.
pub max_inflight_requests_per_router: usize,
```

In `Default::default`, insert after the per-token default:

```rust
max_inflight_requests_per_router: 4,
```

In `log_effective`, insert after the per-token tracing field:

```rust
max_inflight_requests_per_router = self.max_inflight_requests_per_router,
```

- [ ] **Step 4: Add the Junos and SRX CLI contracts**

In `rust-junosmcp/src/cli.rs`, insert after `max_inflight_requests_per_token`:

```rust
/// Max concurrent in-flight requests per target router. 0 = unlimited.
#[arg(
    long,
    env = "JMCP_MAX_INFLIGHT_REQUESTS_PER_ROUTER",
    default_value_t = 4
)]
pub max_inflight_requests_per_router: usize,
```

In `rust-srxmcp/src/cli.rs`, insert the parallel SRX field:

```rust
/// Max concurrent in-flight requests per target router. 0 = unlimited.
#[arg(
    long,
    env = "JMCP_SRX_MAX_INFLIGHT_REQUESTS_PER_ROUTER",
    default_value_t = 4
)]
pub max_inflight_requests_per_router: usize,
```

- [ ] **Step 5: Thread the CLI values into both shared config literals**

In both `main.rs` config literals, insert after `max_inflight_requests_per_token`:

```rust
max_inflight_requests_per_router: args.max_inflight_requests_per_router,
```

- [ ] **Step 6: Run the focused tests and workspace check to verify GREEN**

Run:

```bash
cargo test -p rust-junosmcp-limits config::tests::defaults_are_generous_and_enabled
cargo test -p rust-junosmcp cli::tests::per_router_limit_defaults_and_parses
cargo test -p rust-srxmcp cli::tests::per_router_limit_defaults_and_parses
cargo check --workspace --locked
```

Expected: all three tests pass and the workspace check completes without warnings.

- [ ] **Step 7: Commit the configuration contract**

```bash
git add rust-junosmcp-limits/src/config.rs rust-junosmcp/src/cli.rs rust-junosmcp/src/main.rs rust-srxmcp/src/cli.rs rust-srxmcp/src/main.rs
git commit -m "feat(#147): add per-router limit configuration"
```

---

### Task 2: Protocol-Aware Router Target Extraction

**Files:**
- Create: `rust-junosmcp-limits/src/router.rs`
- Modify: `rust-junosmcp-limits/src/lib.rs:4-8`
- Modify: `rust-junosmcp-limits/Cargo.toml:10-25`
- Generated by Cargo: `Cargo.lock`

**Interfaces:**
- Consumes: Raw buffered JSON-RPC request bytes.
- Produces: `pub(crate) fn extract_router_targets(body: &[u8]) -> Vec<String>`, returning sorted, deduplicated exact router names only for `tools/call` entries.

- [ ] **Step 1: Register the private module and write failing extraction tests**

Add `mod router;` to `rust-junosmcp-limits/src/lib.rs`.

Create `rust-junosmcp-limits/src/router.rs` with these tests only:

```rust
#[cfg(test)]
mod tests {
    use super::extract_router_targets;

    #[test]
    fn extracts_supported_keys_from_single_and_batched_calls() {
        let body = br#"[
            {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"one","arguments":{"router":"r4"}}},
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"two","arguments":{"router_name":"r3"}}},
            {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"three","arguments":{"routers":["r2","r1"]}}},
            {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"four","arguments":{"router_names":"r5"}}}
        ]"#;

        assert_eq!(
            extract_router_targets(body),
            vec!["r1", "r2", "r3", "r4", "r5"]
        );
    }

    #[test]
    fn deduplicates_exact_names_and_sorts_them() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"batch","arguments":{"router":"b","router_name":"a","routers":["b","a","c"]}}}"#;
        assert_eq!(extract_router_targets(body), vec!["a", "b", "c"]);
    }

    #[test]
    fn ignores_non_tools_calls_nested_keys_invalid_types_and_malformed_json() {
        let non_tool = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"arguments":{"router":"r1"}}}"#;
        let nested = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":{"payload":{"router":"nested"},"router":17,"routers":[false,42]}}}"#;

        assert!(extract_router_targets(non_tool).is_empty());
        assert!(extract_router_targets(nested).is_empty());
        assert!(extract_router_targets(b"not-json").is_empty());
    }

    #[test]
    fn preserves_exact_case_and_whitespace() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","arguments":{"routers":["SRX-1","srx-1"," srx-1 "]}}}"#;
        assert_eq!(
            extract_router_targets(body),
            vec![" srx-1 ", "SRX-1", "srx-1"]
        );
    }
}
```

- [ ] **Step 2: Run the extraction tests and verify RED**

Run:

```bash
cargo test -p rust-junosmcp-limits router::tests
```

Expected: compilation fails with an unresolved import for `extract_router_targets`.

- [ ] **Step 3: Add the existing workspace JSON dependency**

Under `[dependencies]` in `rust-junosmcp-limits/Cargo.toml`, add:

```toml
serde_json    = { workspace = true }
```

- [ ] **Step 4: Implement exact, top-level extraction above the test module**

Add to `router.rs`:

```rust
//! Router-target extraction and per-router concurrency primitives.

use serde_json::Value;
use std::collections::BTreeSet;

const ROUTER_KEYS: [&str; 4] = ["router", "router_name", "routers", "router_names"];

/// Return sorted, unique, exact router names from top-level `tools/call`
/// arguments. Invalid protocol input is left for rmcp to diagnose.
pub(crate) fn extract_router_targets(body: &[u8]) -> Vec<String> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };

    let mut targets = BTreeSet::new();
    match &value {
        Value::Array(requests) => {
            for request in requests {
                collect_request_targets(request, &mut targets);
            }
        }
        request => collect_request_targets(request, &mut targets),
    }
    targets.into_iter().collect()
}

fn collect_request_targets(request: &Value, targets: &mut BTreeSet<String>) {
    let Some(request) = request.as_object() else {
        return;
    };
    if request.get("method").and_then(Value::as_str) != Some("tools/call") {
        return;
    }
    let Some(arguments) = request
        .get("params")
        .and_then(Value::as_object)
        .and_then(|params| params.get("arguments"))
        .and_then(Value::as_object)
    else {
        return;
    };

    for key in ROUTER_KEYS {
        if let Some(value) = arguments.get(key) {
            collect_field_targets(value, targets);
        }
    }
}

fn collect_field_targets(value: &Value, targets: &mut BTreeSet<String>) {
    match value {
        Value::String(router) => {
            targets.insert(router.clone());
        }
        Value::Array(routers) => {
            targets.extend(routers.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        _ => {}
    }
}
```

- [ ] **Step 5: Verify extraction and let Cargo update lock metadata**

Run:

```bash
cargo test -p rust-junosmcp-limits router::tests
cargo test -p rust-junosmcp-limits --locked
git diff -- Cargo.lock
```

Expected: four extraction tests pass. Any lockfile diff only adds `serde_json` to the `rust-junosmcp-limits` direct dependency list; no package version changes.

- [ ] **Step 6: Commit the extractor**

```bash
git add Cargo.lock rust-junosmcp-limits/Cargo.toml rust-junosmcp-limits/src/lib.rs rust-junosmcp-limits/src/router.rs
git commit -m "feat(#147): extract router targets from MCP calls"
```

---

### Task 3: Weak Per-Router Semaphore Registry

**Files:**
- Modify: `rust-junosmcp-limits/src/router.rs`

**Interfaces:**
- Consumes: Sorted, unique names from `extract_router_targets` and the configured cap.
- Produces: `RouterLimiter::new(max: usize)` and `RouterLimiter::try_acquire(&[String]) -> Result<Vec<OwnedSemaphorePermit>, String>`; the error string is the saturated exact router name for tracing only.

- [ ] **Step 1: Add failing registry tests to `router.rs`**

Extend the existing test module imports and add these tests:

```rust
use super::RouterLimiter;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn same_router_sheds_while_different_router_is_independent() {
    let limiter = RouterLimiter::new(1);
    let held = limiter.try_acquire(&names(&["r1"])).unwrap();

    assert_eq!(
        limiter.try_acquire(&names(&["r1"])).unwrap_err(),
        "r1"
    );
    let other = limiter.try_acquire(&names(&["r2"])).unwrap();

    drop(other);
    drop(held);
    assert!(limiter.try_acquire(&names(&["r1"])).is_ok());
}

#[test]
fn partial_multi_router_acquisition_rolls_back() {
    let limiter = RouterLimiter::new(1);
    let held_b = limiter.try_acquire(&names(&["b"])).unwrap();

    assert_eq!(
        limiter.try_acquire(&names(&["a", "b"])).unwrap_err(),
        "b"
    );
    assert!(
        limiter.try_acquire(&names(&["a"])).is_ok(),
        "the failed batch must release its already-acquired a permit"
    );
    drop(held_b);
}

#[test]
fn zero_disables_router_permits() {
    let limiter = RouterLimiter::new(0);
    assert!(limiter.try_acquire(&names(&["r1"])).unwrap().is_empty());
    assert!(limiter.try_acquire(&names(&["r1"])).unwrap().is_empty());
}

#[test]
fn weak_registry_reclaims_idle_router_names() {
    let limiter = RouterLimiter::new(1);
    let held = limiter.try_acquire(&names(&["old"])).unwrap();
    assert_eq!(limiter.registry_len(), 1);
    drop(held);

    let replacement = limiter.try_acquire(&names(&["new"])).unwrap();
    assert_eq!(limiter.registry_len(), 1);
    drop(replacement);
}
```

- [ ] **Step 2: Run the registry tests and verify RED**

Run:

```bash
cargo test -p rust-junosmcp-limits router::tests
```

Expected: compilation fails because `RouterLimiter` is not defined.

- [ ] **Step 3: Implement the weak registry above the test module**

Extend imports and add the type:

```rust
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(crate) struct RouterLimiter {
    max: usize,
    semaphores: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
}

impl RouterLimiter {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            semaphores: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn semaphore(&self, router: &str) -> Arc<Semaphore> {
        let mut semaphores = self
            .semaphores
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        semaphores.retain(|_, semaphore| semaphore.strong_count() > 0);

        if let Some(semaphore) = semaphores.get(router).and_then(Weak::upgrade) {
            return semaphore;
        }

        let semaphore = Arc::new(Semaphore::new(self.max.max(1)));
        semaphores.insert(router.to_owned(), Arc::downgrade(&semaphore));
        semaphore
    }

    pub(crate) fn try_acquire(
        &self,
        routers: &[String],
    ) -> Result<Vec<OwnedSemaphorePermit>, String> {
        if self.max == 0 {
            return Ok(Vec::new());
        }

        let mut permits = Vec::with_capacity(routers.len());
        for router in routers {
            match self.semaphore(router).try_acquire_owned() {
                Ok(permit) => permits.push(permit),
                Err(_) => return Err(router.clone()),
            }
        }
        Ok(permits)
    }

    #[cfg(test)]
    fn registry_len(&self) -> usize {
        self.semaphores
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}
```

Replace the prior single `BTreeSet` import rather than importing it twice.

- [ ] **Step 4: Run registry and extractor tests to verify GREEN**

Run:

```bash
cargo test -p rust-junosmcp-limits router::tests
cargo clippy -p rust-junosmcp-limits --all-targets -- -D warnings
```

Expected: all eight router module tests pass and Clippy is clean.

- [ ] **Step 5: Commit the registry**

```bash
git add rust-junosmcp-limits/src/router.rs
git commit -m "feat(#147): add weak per-router permit registry"
```

---

### Task 4: HTTP Enforcement and Destructive-Lease Composition

**Files:**
- Modify: `rust-junosmcp-limits/src/concurrency.rs:1-293`
- Modify: `rust-junosmcp-limits/Cargo.toml:23-25`
- Generated by Cargo: `Cargo.lock`

**Interfaces:**
- Consumes: `extract_router_targets`, `RouterLimiter`, existing `ConcurrencyState`, `GuardedBody`, `overload_response`, and test-only `DeviceLeaseManager`.
- Produces: Router admission in `concurrency_middleware`; body replay; `router_concurrency` 503 responses; preserved streamed-body 413 behavior; real lease-composition coverage.

- [ ] **Step 1: Add the test-only lease dependencies**

Under `[dev-dependencies]` in `rust-junosmcp-limits/Cargo.toml`, add:

```toml
rust-junosmcp-core = { path = "../rust-junosmcp-core" }
tempfile            = { workspace = true }
```

Run `cargo check -p rust-junosmcp-limits --tests` once so Cargo records these direct test dependencies. Inspect `Cargo.lock`; no package version may change.

- [ ] **Step 2: Add test helpers for POST tool calls and bounded handler entry**

In `concurrency.rs`'s test module, retain the existing global/token helpers and add these imports and helpers:

```rust
use axum::body::Bytes;
use axum::routing::post;
use rust_junosmcp_core::DeviceLeaseManager;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::timeout;

fn tool_request(arguments: Value) -> Request<Body> {
    Request::builder()
        .method(axum::http::Method::POST)
        .uri("/mcp")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("mcp-session-id", "test-session")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "test", "arguments": arguments}
            })
            .to_string(),
        ))
        .unwrap()
}

fn blocking_post_router(release: Arc<Notify>, entered: Arc<Notify>) -> Router {
    Router::new().route(
        "/mcp",
        post(move || {
            let release = release.clone();
            let entered = entered.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                "ok"
            }
        }),
    )
}

fn router_state(max_per_router: usize) -> ConcurrencyState {
    ConcurrencyState::new(
        &LimitsConfig {
            max_inflight_requests: 0,
            max_inflight_requests_per_token: 0,
            max_inflight_requests_per_router: max_per_router,
            max_sessions: 0,
            ..Default::default()
        },
        None,
    )
}
```

- [ ] **Step 3: Write failing same-router, isolation, response-lifetime, replay, and streamed-limit tests**

Add:

```rust
#[tokio::test]
async fn per_router_sheds_same_router_and_isolates_different_router() {
    let release = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let app = blocking_post_router(release.clone(), entered.clone()).layer(
        axum::middleware::from_fn_with_state(router_state(1), concurrency_middleware),
    );

    let first_app = app.clone();
    let first = tokio::spawn(async move {
        first_app
            .oneshot(tool_request(json!({"router": "r1"})))
            .await
            .unwrap()
    });
    entered.notified().await;

    let same = timeout(
        Duration::from_millis(200),
        app.clone()
            .oneshot(tool_request(json!({"router_name": "r1"}))),
    )
    .await
    .expect("same-router request queued instead of being shed")
    .unwrap();
    assert_eq!(same.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(same.headers().get("retry-after").unwrap(), "1");
    let body = axum::body::to_bytes(same.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"error": "overloaded", "limit": "router_concurrency"})
    );

    let other_app = app.clone();
    let other = tokio::spawn(async move {
        other_app
            .oneshot(tool_request(json!({"router": "r2"})))
            .await
            .unwrap()
    });
    entered.notified().await;

    release.notify_waiters();
    let first = first.await.unwrap();
    let other = other.await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(other.status(), StatusCode::OK);
    drop(first);
    drop(other);
}

#[tokio::test]
async fn router_permit_lives_until_response_body_is_dropped() {
    let app = Router::new()
        .route("/mcp", post(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            router_state(1),
            concurrency_middleware,
        ));

    let first = app
        .clone()
        .oneshot(tool_request(json!({"router": "r1"})))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let shed = app
        .clone()
        .oneshot(tool_request(json!({"router": "r1"})))
        .await
        .unwrap();
    assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(first);
    let admitted = app
        .oneshot(tool_request(json!({"router": "r1"})))
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
}

#[tokio::test]
async fn malformed_json_is_replayed_unchanged() {
    let app = Router::new()
        .route("/mcp", post(|body: Bytes| async move { body }))
        .layer(axum::middleware::from_fn_with_state(
            router_state(1),
            concurrency_middleware,
        ));
    let original = Bytes::from_static(b"not-json");
    let request = Request::builder()
        .method(axum::http::Method::POST)
        .uri("/mcp")
        .body(Body::from(original.clone()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let replayed = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(replayed, original);
}

#[tokio::test]
async fn streamed_body_over_outer_limit_stays_413() {
    let cfg = LimitsConfig {
        max_request_body_bytes: 8,
        max_inflight_requests: 0,
        max_inflight_requests_per_token: 0,
        max_inflight_requests_per_router: 1,
        max_sessions: 0,
        ..Default::default()
    };
    let app = Router::new()
        .route("/mcp", post(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            ConcurrencyState::new(&cfg, None),
            concurrency_middleware,
        ));
    let app = apply_body_limit(app, &cfg);
    let stream = futures::stream::iter([Ok::<_, Infallible>(Bytes::from_static(
        b"more-than-eight-bytes",
    ))]);
    let request = Request::builder()
        .method(axum::http::Method::POST)
        .uri("/mcp")
        .body(Body::from_stream(stream))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
```

- [ ] **Step 4: Write the failing real lease-composition test**

Add:

```rust
#[tokio::test]
async fn router_limit_composes_with_real_destructive_lease() {
    let directory = tempfile::tempdir().unwrap();
    let leases = Arc::new(
        DeviceLeaseManager::with_timing(
            directory.path(),
            Duration::from_secs(2),
            Duration::from_millis(10),
        )
        .unwrap(),
    );
    let external = leases.acquire("r1", "external", "external-1").await.unwrap();
    let entered = Arc::new(Notify::new());

    let app = Router::new()
        .route(
            "/mcp",
            post({
                let leases = leases.clone();
                let entered = entered.clone();
                move || {
                    let leases = leases.clone();
                    let entered = entered.clone();
                    async move {
                        entered.notify_one();
                        let _lease = leases
                            .acquire("r1", "http-destructive", "http-1")
                            .await
                            .unwrap();
                        "ok"
                    }
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            router_state(1),
            concurrency_middleware,
        ));

    let first_app = app.clone();
    let first = tokio::spawn(async move {
        first_app
            .oneshot(tool_request(json!({"router": "r1"})))
            .await
            .unwrap()
    });
    entered.notified().await;

    let shed = timeout(
        Duration::from_millis(200),
        app.clone()
            .oneshot(tool_request(json!({"router": "r1"}))),
    )
    .await
    .expect("second request entered the lease wait instead of being shed")
    .unwrap();
    assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(external);
    let first = timeout(Duration::from_secs(1), first)
        .await
        .expect("first request deadlocked after lease release")
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();

    let admitted = app
        .oneshot(tool_request(json!({"router": "r1"})))
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
}
```

- [ ] **Step 5: Run the primary middleware test and verify RED**

Run:

```bash
cargo test -p rust-junosmcp-limits concurrency::tests::per_router_sheds_same_router_and_isolates_different_router -- --nocapture
```

Expected: the test fails within 200 ms with `same-router request queued instead of being shed`, proving the existing middleware does not enforce router capacity.

- [ ] **Step 6: Add router state and buffer/replay enforcement**

At the top of `concurrency.rs`, update imports:

```rust
use crate::router::{extract_router_targets, RouterLimiter};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
```

Extend `ConcurrencyState`:

```rust
per_router: RouterLimiter,
max_per_router: usize,
```

Initialize those fields in `ConcurrencyState::new`:

```rust
per_router: RouterLimiter::new(cfg.max_inflight_requests_per_router),
max_per_router: cfg.max_inflight_requests_per_router,
```

Add this private helper before `is_session_creating`:

```rust
async fn inspect_router_targets(req: Request) -> Result<(Request, Vec<String>), Response> {
    if req.method() != Method::POST {
        return Ok((req, Vec::new()));
    }

    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(error = %error, "request body rejected while extracting router targets");
            return Err(StatusCode::PAYLOAD_TOO_LARGE.into_response());
        }
    };
    let targets = extract_router_targets(&bytes);
    Ok((Request::from_parts(parts, Body::from(bytes)), targets))
}
```

Change the middleware parameter to `mut req: Request`. After the existing session-cap check and before `next.run(req)`, add:

```rust
if state.max_per_router > 0 {
    let (rebuilt, routers) = match inspect_router_targets(req).await {
        Ok(result) => result,
        Err(response) => return response,
    };
    req = rebuilt;

    match state.per_router.try_acquire(&routers) {
        Ok(mut router_permits) => permits.append(&mut router_permits),
        Err(router) => {
            tracing::warn!(
                limit = "router_concurrency",
                router = %router,
                max = state.max_per_router,
                "request shed"
            );
            return overload_response("router_concurrency");
        }
    }
}
```

Update the module-level and middleware doc comments from “global + per-token” to “global + per-token + per-router.”

- [ ] **Step 7: Run all shared-crate tests and verify GREEN**

Run:

```bash
cargo test -p rust-junosmcp-limits --locked -- --nocapture
cargo clippy -p rust-junosmcp-limits --all-targets -- -D warnings
```

Expected: extraction, registry, same-router, isolation, response-lifetime, malformed replay, streamed 413, and real lease-composition tests all pass; Clippy emits no warnings.

- [ ] **Step 8: Inspect lockfile and commit HTTP enforcement**

Run `git diff -- Cargo.lock` and confirm only direct dependency metadata changed. Then:

```bash
git add Cargo.lock rust-junosmcp-limits/Cargo.toml rust-junosmcp-limits/src/concurrency.rs
git commit -m "feat(#147): enforce per-router HTTP concurrency"
```

---

### Task 5: User Documentation and Changelogs

**Files:**
- Modify: `README.md:559-578`
- Modify: `CHANGELOG.md:7-24`
- Modify: `rust-srxmcp/CHANGELOG.md:9-17`

**Interfaces:**
- Consumes: Final CLI names, env names, default, overload kind, batch accounting, and lease order from Tasks 1-4.
- Produces: Operator-facing tuning and compatibility guidance for both binaries.

- [ ] **Step 1: Add a documentation-contract check that initially fails**

Run:

```bash
rg -n "max-inflight-requests-per-router|router_concurrency|JMCP_SRX_MAX_INFLIGHT_REQUESTS_PER_ROUTER" README.md CHANGELOG.md rust-srxmcp/CHANGELOG.md
```

Expected: no matches, demonstrating the new public control is undocumented.

- [ ] **Step 2: Extend the README resource-limit table**

Insert after the per-token row:

```markdown
| `--max-inflight-requests-per-router` | `JMCP_MAX_INFLIGHT_REQUESTS_PER_ROUTER` / `JMCP_SRX_MAX_INFLIGHT_REQUESTS_PER_ROUTER` | 4 | Per-router concurrency cap → **503** |
```

Replace the existing permit/deferred paragraphs with:

```markdown
Over-limit responses carry `Retry-After: 1`. Concurrency permits are released when
the response stream ends. A multi-router call holds one router slot for each unique
top-level `router`, `router_name`, `routers`, or `router_names` target.

The per-router HTTP permit is acquired before a destructive workflow waits for its
cross-process device lease. A destructive call counts once while waiting for or
holding that lease; the HTTP cap bounds both reads and destructive waiters, while the
lease remains the authority that serializes destructive operations across processes.

**Deferred (follow-ups on #131):** per-token session caps, a Prometheus `/metrics`
endpoint, and RPS rate-limiting.
```

- [ ] **Step 3: Add root and SRX Unreleased entries**

Under each `## [Unreleased]`, add an `### Added` section before `### Fixed`:

```markdown
### Added

- **#147 - per-router HTTP concurrency limits.** Both streamable-HTTP endpoints
  now cap concurrent work per exact router name at 4 by default (`0` disables),
  with immediate `503` + `Retry-After: 1` load shedding. Multi-router calls hold
  one slot per unique target, and destructive calls count once while waiting for
  or holding the existing cross-process device lease.
```

- [ ] **Step 4: Verify documentation names match the CLI**

Run:

```bash
rg -n "max-inflight-requests-per-router|JMCP_MAX_INFLIGHT_REQUESTS_PER_ROUTER|JMCP_SRX_MAX_INFLIGHT_REQUESTS_PER_ROUTER|router_concurrency" README.md CHANGELOG.md rust-srxmcp/CHANGELOG.md rust-junosmcp/src/cli.rs rust-srxmcp/src/cli.rs rust-junosmcp-limits/src/concurrency.rs
git diff --check
```

Expected: flag/env names are identical across code and docs; no whitespace errors.

- [ ] **Step 5: Commit documentation**

```bash
git add README.md CHANGELOG.md rust-srxmcp/CHANGELOG.md
git commit -m "docs(#147): document per-router HTTP limits"
```

---

### Task 6: Full Offline Verification and Handoff Evidence

**Files:**
- Verify only: all files changed in Tasks 1-5
- Modify only if a verification command identifies a concrete issue: the file causing that issue

**Interfaces:**
- Consumes: Complete issue #147 implementation.
- Produces: Evidence for formatting, lint, tests, guards, CLI exposure, security, release readiness, compatibility, and skipped live checks.

- [ ] **Step 1: Verify formatting and literal diffs**

Run:

```bash
cargo fmt --all --check
git diff --check
```

Expected: both exit 0. If formatting fails, run `cargo fmt --all`, inspect the exact diff, rerun both checks, and commit only the formatting correction.

- [ ] **Step 2: Run workspace lint**

Run the underlying `just lint` recipe because `just` is unavailable on this workstation:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: exit 0 with no warnings.

- [ ] **Step 3: Run the complete locked offline suite**

Run the underlying `just test` recipe:

```bash
cargo test --workspace --locked
```

Expected: all non-ignored tests pass. Record the exact passed/failed/ignored summary; real-device tests remain ignored.

- [ ] **Step 4: Run the guard and endpoint help equivalents**

`just guard` is the lint plus test pair already run in Steps 2-3. Run the underlying `just e2e` commands explicitly:

```bash
cargo run -p rust-junosmcp -- --help >/dev/null
cargo run -p rust-srxmcp -- --help >/dev/null
```

Expected: both binaries exit 0. Also run visible assertions:

```bash
cargo run -q -p rust-junosmcp -- --help | rg -- "--max-inflight-requests-per-router"
cargo run -q -p rust-srxmcp -- --help | rg -- "--max-inflight-requests-per-router"
```

Expected: each help output contains the new flag.

- [ ] **Step 5: Run security and release-check equivalents**

Run the underlying `just security` recipe:

```bash
trivy fs --scanners vuln,misconfig,secret --exit-code 1 .
```

Expected: exit 0 with no vulnerability, misconfiguration, or secret finding that violates repository policy.

The underlying `just release-check` is `fmt`, `lint`, `test`, and `security`; confirm Steps 1-3 and this step are all green. Do not claim `just release-check` itself ran while the runner is unavailable.

- [ ] **Step 6: Review dependency, compatibility, and repository state**

Run:

```bash
cargo tree -p rust-junosmcp-limits
git diff origin/main...HEAD --stat
git diff origin/main...HEAD -- Cargo.lock
git status --short --branch
```

Expected:

- no new external runtime package version;
- lockfile changes only record new direct dependencies for the existing limits crate;
- no MCP schema, annotation, auth-scope, audit-field, core workflow, or device-I/O file changed;
- the worktree is clean on `agent/issue-147-per-router-limits`.

- [ ] **Step 7: Record skipped live checks and remaining risk**

Handoff must explicitly state:

```text
Skipped: just integration and all ignored real-device tests; CONFIRM_LAB_INTEGRATION was not set and no device was contacted.
Compatibility: stdio, MCP schemas, annotations, auth scopes, audit fields, overload formats, and device lease semantics are unchanged.
Remaining risk: router targeting is derived from top-level JSON arguments before rmcp schema validation; malformed inputs are replayed, while valid future tools must continue using one of the four documented router keys to receive per-router admission control.
```

- [ ] **Step 8: Commit any verification-only correction**

If Steps 1-6 required a code or documentation correction, rerun its focused RED/GREEN regression where applicable, then:

```bash
git add -u
git commit -m "fix(#147): address final verification finding"
```

If no correction was required, do not create an empty commit.
