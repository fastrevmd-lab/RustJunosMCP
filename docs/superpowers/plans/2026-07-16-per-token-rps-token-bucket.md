# Per-Token RPS Token-Bucket Rate Limiting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, exact per-authenticated-token token bucket to both streamable-HTTP binaries, returning stable HTTP 429 responses with retry guidance while preserving every existing 503 concurrency/session contract.

**Architecture:** Extend the shared `LimitsConfig`, then add a focused `rate_limit.rs` with fixed-point bucket state and one public router helper. Authentication remains outside the rate layer, while rate limiting sits outside the existing concurrency/session middleware so over-rate requests consume no concurrency permit. Both binaries use the same shared implementation and matching contract tests.

**Tech Stack:** Rust 2021, Axum 0.8 middleware/router layers, `DashMap`, `std::time::Instant`, existing `metrics`/Prometheus support, Clap, Tokio, and the existing real-binary HTTP harnesses.

## Global Constraints

- Work only in `.worktrees/issue-150-per-token-rate-limit` on branch `issue-150-per-token-rate-limit`.
- Follow `docs/superpowers/specs/2026-07-16-per-token-rps-token-bucket-design.md` exactly.
- The limiter defaults to disabled with `rate = 0` and `burst = 0`; exactly one zero is a startup error.
- Rate and burst are positive whole-number `u64` values when enabled.
- Each authenticated `/mcp` HTTP request costs exactly one token; downstream failure or cancellation does not refund it.
- No-auth mode, `/metrics`, and stdio remain outside per-token rate limiting.
- Rate limiting must run after auth and before all concurrency/session checks.
- Rate exhaustion returns exact HTTP 429, `Retry-After`, `Content-Type: application/json`, and `{"error":"rate_limited","limit":"token_rate"}`.
- Existing 503 statuses, bodies, headers, and metric labels remain byte-for-byte compatible.
- Do not add a dependency or change `Cargo.lock`.
- Do not add token names or bearer secrets to metric labels.
- Use test-driven development: observe each focused test fail for the intended reason before implementing it.
- Do not run ignored device/network tests without `CONFIRM_LAB_INTEGRATION=yes`.

## File Structure

- Create `rust-junosmcp-limits/src/rate_limit.rs`: fixed-point bucket arithmetic, per-token registry, Axum middleware, router helper, and focused unit/middleware tests.
- Modify `rust-junosmcp-limits/src/config.rs`: two knobs, typed pair-validation error, enabled predicate, startup logging, and configuration tests.
- Modify `rust-junosmcp-limits/src/overload.rs`: dedicated stable 429 response and metric recording; retain the 503 helper unchanged.
- Modify `rust-junosmcp-limits/src/lib.rs`: register the rate module and export only `apply_token_rate_limit` plus the config error.
- Modify `rust-junosmcp/src/cli.rs` and `rust-srxmcp/src/cli.rs`: matching CLI/env surfaces and parser tests.
- Modify `rust-junosmcp/src/main.rs` and `rust-srxmcp/src/main.rs`: forward both CLI values into `LimitsConfig`.
- Modify `rust-junosmcp/src/http_transport.rs` and `rust-srxmcp/src/http_transport.rs`: validate config before binding and install the rate layer between auth and concurrency.
- Modify both `tests/http_limits.rs`: real-binary 429 contract parity tests.
- Modify `README.md`, `docs/METRICS.md`, `CHANGELOG.md`, and `rust-srxmcp/CHANGELOG.md`: operator knobs, semantics, metric label, usage guidance, and release notes.

---

### Task 1: Configuration Contract and CLI Parity

**Files:**
- Modify: `rust-junosmcp-limits/src/config.rs:3-98`
- Modify: `rust-junosmcp-limits/src/lib.rs:11-15`
- Modify: `rust-junosmcp/src/cli.rs:101-139,217-257`
- Modify: `rust-srxmcp/src/cli.rs:76-122,148-189`
- Modify: `rust-junosmcp/src/main.rs:230-239`
- Modify: `rust-srxmcp/src/main.rs:193-202`
- Modify: `rust-junosmcp/src/http_transport.rs:57-64`
- Modify: `rust-srxmcp/src/http_transport.rs:98-104`

**Interfaces:**
- Consumes: existing `LimitsConfig`, Clap CLI structs, and `anyhow::Context` already imported by both HTTP transports.
- Produces: `LimitsConfig::{max_requests_per_second_per_token, max_request_burst_per_token}`, `LimitsConfig::validate() -> Result<(), LimitsConfigError>`, `LimitsConfig::token_rate_limit_enabled() -> bool`, and public `LimitsConfigError::IncompleteTokenRateLimit { rate, burst }`.

- [ ] **Step 1: Add failing shared configuration tests**

Append these tests in `rust-junosmcp-limits/src/config.rs` before adding the fields or methods:

```rust
#[test]
fn token_rate_defaults_disabled_and_valid() {
    let config = LimitsConfig::default();
    assert_eq!(config.max_requests_per_second_per_token, 0);
    assert_eq!(config.max_request_burst_per_token, 0);
    assert!(!config.token_rate_limit_enabled());
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn token_rate_requires_rate_and_burst_together() {
    for (rate, burst) in [(5, 0), (0, 8)] {
        let config = LimitsConfig {
            max_requests_per_second_per_token: rate,
            max_request_burst_per_token: burst,
            ..Default::default()
        };
        assert_eq!(
            config.validate(),
            Err(LimitsConfigError::IncompleteTokenRateLimit { rate, burst })
        );
        assert!(!config.token_rate_limit_enabled());
    }

    let enabled = LimitsConfig {
        max_requests_per_second_per_token: 5,
        max_request_burst_per_token: 8,
        ..Default::default()
    };
    assert_eq!(enabled.validate(), Ok(()));
    assert!(enabled.token_rate_limit_enabled());
}
```

- [ ] **Step 2: Add failing CLI parser tests in both binaries**

Add this test to each CLI test module, changing only the binary name in `parse_from`:

```rust
#[test]
fn per_token_rate_limit_defaults_and_parses() {
    let default_cli = Cli::parse_from(["rust-junosmcp"]);
    assert_eq!(default_cli.max_requests_per_second_per_token, 0);
    assert_eq!(default_cli.max_request_burst_per_token, 0);

    let custom = Cli::parse_from([
        "rust-junosmcp",
        "--max-requests-per-second-per-token",
        "7",
        "--max-request-burst-per-token",
        "11",
    ]);
    assert_eq!(custom.max_requests_per_second_per_token, 7);
    assert_eq!(custom.max_request_burst_per_token, 11);
}
```

For `rust-srxmcp/src/cli.rs`, use `"rust-srxmcp"` in both arrays.

- [ ] **Step 3: Run the tests and verify the intended compile failures**

Run:

```bash
cargo test -p rust-junosmcp-limits config::tests --locked
cargo test -p rust-junosmcp cli::tests::per_token_rate_limit_defaults_and_parses --locked
cargo test -p rust-srxmcp cli::tests::per_token_rate_limit_defaults_and_parses --locked
```

Expected: FAIL with missing `LimitsConfig`/`Cli` fields and missing validation methods, not an unrelated baseline failure.

- [ ] **Step 4: Implement the typed shared configuration contract**

In `rust-junosmcp-limits/src/config.rs`, add the two fields after the existing per-token concurrency field, default both to zero, and add this error and behavior:

```rust
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitsConfigError {
    IncompleteTokenRateLimit { rate: u64, burst: u64 },
}

impl fmt::Display for LimitsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteTokenRateLimit { rate, burst } => write!(
                f,
                "per-token request rate and burst must both be zero (disabled) or both be positive (rate={rate}, burst={burst})"
            ),
        }
    }
}

impl std::error::Error for LimitsConfigError {}
```

The new struct/default members and methods are:

```rust
/// Max requests per second per bearer token. `0` disables with burst `0`.
pub max_requests_per_second_per_token: u64,
/// Max immediate request burst per bearer token. `0` disables with rate `0`.
pub max_request_burst_per_token: u64,

// In Default::default()
max_requests_per_second_per_token: 0,
max_request_burst_per_token: 0,

pub fn validate(&self) -> Result<(), LimitsConfigError> {
    let rate = self.max_requests_per_second_per_token;
    let burst = self.max_request_burst_per_token;
    if (rate == 0) != (burst == 0) {
        return Err(LimitsConfigError::IncompleteTokenRateLimit { rate, burst });
    }
    Ok(())
}

pub fn token_rate_limit_enabled(&self) -> bool {
    self.max_requests_per_second_per_token > 0 && self.max_request_burst_per_token > 0
}
```

Add both values to `log_effective!`, and re-export the error in `lib.rs`:

```rust
pub use config::{LimitsConfig, LimitsConfigError};
```

- [ ] **Step 5: Add matching CLI fields and forward them**

Add to `rust-junosmcp/src/cli.rs`:

```rust
/// Max requests per second per bearer token. Set with burst; 0/0 = disabled.
#[arg(
    long,
    env = "JMCP_MAX_REQUESTS_PER_SECOND_PER_TOKEN",
    default_value_t = 0
)]
pub max_requests_per_second_per_token: u64,

/// Max immediate request burst per bearer token. Set with rate; 0/0 = disabled.
#[arg(long, env = "JMCP_MAX_REQUEST_BURST_PER_TOKEN", default_value_t = 0)]
pub max_request_burst_per_token: u64,
```

Add the same fields to `rust-srxmcp/src/cli.rs` with environment names
`JMCP_SRX_MAX_REQUESTS_PER_SECOND_PER_TOKEN` and
`JMCP_SRX_MAX_REQUEST_BURST_PER_TOKEN`.

Forward both fields in each main `LimitsConfig` literal:

```rust
max_requests_per_second_per_token: args.max_requests_per_second_per_token,
max_request_burst_per_token: args.max_request_burst_per_token,
```

- [ ] **Step 6: Enforce validation before either listener can bind**

Immediately before `limits.log_effective()` in Junos `serve` and SRX
`serve_inner`, add:

```rust
limits
    .validate()
    .context("validating HTTP resource limits")?;
```

This uses the existing `anyhow::Context` import and makes direct transport
callers enforce the same startup contract as the binaries.

- [ ] **Step 7: Format and run focused green tests**

Run:

```bash
cargo fmt --all
cargo test -p rust-junosmcp-limits config::tests --locked
cargo test -p rust-junosmcp cli::tests::per_token_rate_limit_defaults_and_parses --locked
cargo test -p rust-srxmcp cli::tests::per_token_rate_limit_defaults_and_parses --locked
cargo check -p rust-junosmcp -p rust-srxmcp --locked
```

Expected: all focused tests pass and both binaries check successfully.

- [ ] **Step 8: Commit configuration parity**

```bash
git add rust-junosmcp-limits/src/config.rs rust-junosmcp-limits/src/lib.rs \
  rust-junosmcp/src/cli.rs rust-junosmcp/src/main.rs rust-junosmcp/src/http_transport.rs \
  rust-srxmcp/src/cli.rs rust-srxmcp/src/main.rs rust-srxmcp/src/http_transport.rs
git commit -m "feat(150): add per-token rate configuration"
```

---

### Task 2: Exact Fixed-Point Token Bucket Core

**Files:**
- Create: `rust-junosmcp-limits/src/rate_limit.rs`
- Modify: `rust-junosmcp-limits/src/lib.rs:4-10`

**Interfaces:**
- Consumes: validated `LimitsConfig` fields from Task 1, `CallerCtx.token_name` only in the later middleware task.
- Produces internally: `TokenRateLimitState::new(&LimitsConfig)`, `TokenRateLimitState::check_at(&str, Instant) -> RateDecision`, and `RateDecision::{Allowed, Limited { retry_after_secs }}`.

- [ ] **Step 1: Register the module and create failing bucket tests**

Add `mod rate_limit;` to `lib.rs`. Create `rate_limit.rs` initially with imports,
the scale constant, and the tests below so compilation fails on the not-yet-defined
state/decision types:

```rust
//! Per-authenticated-token request-rate limiting.

use crate::config::LimitsConfig;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOKEN_SCALE: u128 = 1_000_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    fn state(rate: u64, burst: u64) -> TokenRateLimitState {
        TokenRateLimitState::new(&LimitsConfig {
            max_requests_per_second_per_token: rate,
            max_request_burst_per_token: burst,
            ..Default::default()
        })
    }

    #[test]
    fn fresh_bucket_admits_exact_burst_then_limits() {
        let state = state(2, 3);
        let now = Instant::now();
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert_eq!(
            state.check_at("alice", now),
            RateDecision::Limited { retry_after_secs: 1 }
        );
    }

    #[test]
    fn partial_refill_reaches_exact_token_boundary() {
        let state = state(2, 1);
        let start = Instant::now();
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        assert_eq!(
            state.check_at("alice", start + Duration::from_millis(250)),
            RateDecision::Limited { retry_after_secs: 1 }
        );
        assert_eq!(
            state.check_at("alice", start + Duration::from_millis(500)),
            RateDecision::Allowed
        );
    }

    #[test]
    fn long_idle_refill_is_capped_at_burst() {
        let state = state(4, 2);
        let start = Instant::now();
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
        let later = start + Duration::from_secs(60);
        assert_eq!(state.check_at("alice", later), RateDecision::Allowed);
        assert_eq!(state.check_at("alice", later), RateDecision::Allowed);
        assert_eq!(
            state.check_at("alice", later),
            RateDecision::Limited { retry_after_secs: 1 }
        );
    }

    #[test]
    fn token_names_are_isolated() {
        let state = state(1, 1);
        let now = Instant::now();
        assert_eq!(state.check_at("alice", now), RateDecision::Allowed);
        assert!(matches!(state.check_at("alice", now), RateDecision::Limited { .. }));
        assert_eq!(state.check_at("bob", now), RateDecision::Allowed);
    }

    #[test]
    fn concurrent_checks_admit_exactly_the_burst() {
        const BURST: usize = 8;
        let state = Arc::new(state(1, BURST as u64));
        let barrier = Arc::new(std::sync::Barrier::new(BURST * 2));
        let now = Instant::now();
        let admitted = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..BURST * 2)
                .map(|_| {
                    let state = state.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        state.check_at("alice", now) == RateDecision::Allowed
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter(|handle| handle.join().unwrap())
                .count()
        });
        assert_eq!(admitted, BURST);
    }

    #[test]
    fn refill_arithmetic_saturates() {
        assert_eq!(refill_units(Duration::MAX, u64::MAX), u128::MAX);
    }
}
```

- [ ] **Step 2: Run the new module tests and verify red**

Run:

```bash
cargo test -p rust-junosmcp-limits rate_limit::tests --locked
```

Expected: FAIL to compile because `TokenRateLimitState`, `RateDecision`, and
`refill_units` are not defined.

- [ ] **Step 3: Implement the exact bucket core**

Add this production code above the tests in `rate_limit.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateDecision {
    Allowed,
    Limited { retry_after_secs: u64 },
}

#[derive(Debug)]
struct Bucket {
    available_units: u128,
    last_refill: Instant,
}

impl Bucket {
    fn full(burst: u64, now: Instant) -> Self {
        Self {
            available_units: capacity_units(burst),
            last_refill: now,
        }
    }

    fn check(&mut self, now: Instant, rate: u64, burst: u64) -> RateDecision {
        if let Some(elapsed) = now.checked_duration_since(self.last_refill) {
            self.available_units = self
                .available_units
                .saturating_add(refill_units(elapsed, rate))
                .min(capacity_units(burst));
            self.last_refill = now;
        }

        if self.available_units >= TOKEN_SCALE {
            self.available_units -= TOKEN_SCALE;
            return RateDecision::Allowed;
        }

        let deficit_units = TOKEN_SCALE - self.available_units;
        let wait_ns = deficit_units.div_ceil(u128::from(rate));
        let retry_secs = wait_ns.div_ceil(TOKEN_SCALE).max(1);
        RateDecision::Limited {
            retry_after_secs: u64::try_from(retry_secs).unwrap_or(u64::MAX),
        }
    }
}

#[derive(Clone)]
struct TokenRateLimitState {
    buckets: Arc<DashMap<String, Bucket>>,
    rate_per_second: u64,
    burst: u64,
}

impl TokenRateLimitState {
    fn new(config: &LimitsConfig) -> Self {
        debug_assert!(config.token_rate_limit_enabled());
        Self {
            buckets: Arc::new(DashMap::new()),
            rate_per_second: config.max_requests_per_second_per_token,
            burst: config.max_request_burst_per_token,
        }
    }

    fn check_at(&self, token: &str, now: Instant) -> RateDecision {
        let mut bucket = self
            .buckets
            .entry(token.to_owned())
            .or_insert_with(|| Bucket::full(self.burst, now));
        bucket.check(now, self.rate_per_second, self.burst)
    }
}

fn capacity_units(burst: u64) -> u128 {
    u128::from(burst).saturating_mul(TOKEN_SCALE)
}

fn refill_units(elapsed: Duration, rate: u64) -> u128 {
    elapsed.as_nanos().saturating_mul(u128::from(rate))
}

```

The positive denominator invariant comes from Task 1 validation plus the
`debug_assert!` in `new`.

- [ ] **Step 4: Add a clock-rewind/no-double-refill regression test**

Append:

```rust
#[test]
fn earlier_instant_does_not_move_refill_clock_backward() {
    let state = state(2, 1);
    let start = Instant::now();
    assert_eq!(state.check_at("alice", start), RateDecision::Allowed);
    assert!(matches!(
        state.check_at("alice", start + Duration::from_millis(250)),
        RateDecision::Limited { .. }
    ));
    assert!(matches!(
        state.check_at("alice", start),
        RateDecision::Limited { .. }
    ));
    assert_eq!(
        state.check_at("alice", start + Duration::from_millis(500)),
        RateDecision::Allowed
    );
}
```

- [ ] **Step 5: Format and run focused green tests**

Run:

```bash
cargo fmt --all
cargo test -p rust-junosmcp-limits rate_limit::tests --locked
cargo clippy -p rust-junosmcp-limits --all-targets -- -D warnings
```

Expected: all bucket tests pass and the crate is warning-free.

- [ ] **Step 6: Commit the bucket core**

```bash
git add rust-junosmcp-limits/src/lib.rs rust-junosmcp-limits/src/rate_limit.rs
git commit -m "feat(150): implement exact per-token token bucket"
```

---

### Task 3: Stable 429 Response, Middleware, Metrics, and Composition

**Files:**
- Modify: `rust-junosmcp-limits/src/overload.rs:1-94`
- Modify: `rust-junosmcp-limits/src/rate_limit.rs`
- Modify: `rust-junosmcp-limits/src/lib.rs:11-15`

**Interfaces:**
- Consumes: `TokenRateLimitState` and `RateDecision` from Task 2, existing `CallerCtx`, existing `ConcurrencyState`/`concurrency_middleware`, and `prometheus::record_limit_hit`.
- Produces: public `apply_token_rate_limit(router: axum::Router, config: &LimitsConfig) -> axum::Router`; crate-private `rate_limited_response(retry_after_secs: u64) -> Response`.

- [ ] **Step 1: Add a failing exact response/metric test**

In `overload.rs`, import `CONTENT_TYPE` alongside `RETRY_AFTER`, then add:

```rust
#[tokio::test(flavor = "current_thread")]
async fn rate_limited_response_has_stable_contract_and_metric() {
    let (recorder, handle) = crate::prometheus::test_recorder("junos");
    let response = metrics::with_local_recorder(&recorder, || rate_limited_response(3));

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get(RETRY_AFTER).unwrap(), "3");
    assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "application/json");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        body.as_ref(),
        br#"{"error":"rate_limited","limit":"token_rate"}"#
    );

    handle.run_upkeep();
    let text = handle.render();
    let sample = text
        .lines()
        .find(|line| {
            line.starts_with("junosmcp_limit_hits_total{")
                && line.contains("limit=\"token_rate\"")
                && line.contains("event=\"request_rejected\"")
        })
        .expect("token-rate rejection metric");
    assert!(sample.ends_with(" 1"), "unexpected sample: {sample}");
    assert!(!sample.contains("token="));
}
```

- [ ] **Step 2: Add failing middleware behavior tests**

Extend the `rate_limit.rs` test module imports with Axum/Tower test types and add
these helpers/tests:

```rust
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use rust_junosmcp_auth::caller::CallerCtx;
use rust_junosmcp_auth::ScopeSet;
use tokio::sync::Notify;
use tower::ServiceExt as _;

fn caller(name: &str) -> CallerCtx {
    CallerCtx {
        token_name: name.to_owned(),
        routers: ScopeSet::Wildcard,
        tools: ScopeSet::Wildcard,
    }
}

fn request(token: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::empty())
        .unwrap();
    if let Some(token) = token {
        request.extensions_mut().insert(caller(token));
    }
    request
}

#[tokio::test]
async fn middleware_returns_exact_429_and_isolates_tokens() {
    let config = LimitsConfig {
        max_requests_per_second_per_token: 1,
        max_request_burst_per_token: 1,
        ..Default::default()
    };
    let app = apply_token_rate_limit(
        Router::new().route("/", post(|| async { StatusCode::OK })),
        &config,
    );

    assert_eq!(
        app.clone().oneshot(request(Some("alice"))).await.unwrap().status(),
        StatusCode::OK
    );
    let limited = app
        .clone()
        .oneshot(request(Some("alice")))
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers().get(header::RETRY_AFTER).unwrap(), "1");
    assert_eq!(
        limited.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let body = to_bytes(limited.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        body.as_ref(),
        br#"{"error":"rate_limited","limit":"token_rate"}"#
    );

    assert_eq!(
        app.clone().oneshot(request(Some("bob"))).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone().oneshot(request(None)).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        app.oneshot(request(None)).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn rate_limit_precedes_global_concurrency_but_preserves_503() {
    let config = LimitsConfig {
        max_requests_per_second_per_token: 1,
        max_request_burst_per_token: 1,
        max_inflight_requests: 1,
        max_inflight_requests_per_token: 0,
        max_inflight_requests_per_router: 0,
        ..Default::default()
    };
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let handler = {
        let entered = entered.clone();
        let release = release.clone();
        move || {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                StatusCode::OK
            }
        }
    };
    let concurrency = crate::ConcurrencyState::new(&config, None);
    let app = Router::new()
        .route("/", post(handler))
        .layer(axum::middleware::from_fn_with_state(
            concurrency,
            crate::concurrency_middleware,
        ));
    let app = apply_token_rate_limit(app, &config);

    let first_app = app.clone();
    let first = tokio::spawn(async move {
        first_app.oneshot(request(Some("alice"))).await.unwrap()
    });
    entered.notified().await;

    let alice = app
        .clone()
        .oneshot(request(Some("alice")))
        .await
        .unwrap();
    assert_eq!(alice.status(), StatusCode::TOO_MANY_REQUESTS);

    let bob = app.clone().oneshot(request(Some("bob"))).await.unwrap();
    assert_eq!(bob.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(bob.headers().get(header::RETRY_AFTER).unwrap(), "1");
    let body = to_bytes(bob.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        body.as_ref(),
        br#"{"error":"overloaded","limit":"global_concurrency"}"#
    );

    release.notify_one();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn cancellation_does_not_refund_consumed_rate_token() {
    let config = LimitsConfig {
        max_requests_per_second_per_token: 1,
        max_request_burst_per_token: 1,
        ..Default::default()
    };
    let entered = Arc::new(Notify::new());
    let never_release = Arc::new(Notify::new());
    let handler = {
        let entered = entered.clone();
        let never_release = never_release.clone();
        move || {
            let entered = entered.clone();
            let never_release = never_release.clone();
            async move {
                entered.notify_one();
                never_release.notified().await;
                StatusCode::OK
            }
        }
    };
    let app = apply_token_rate_limit(Router::new().route("/", post(handler)), &config);
    let first_app = app.clone();
    let first = tokio::spawn(async move {
        first_app.oneshot(request(Some("alice"))).await.unwrap()
    });
    entered.notified().await;
    first.abort();
    let _ = first.await;

    let second = app.oneshot(request(Some("alice"))).await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn downstream_error_does_not_refund_consumed_rate_token() {
    let config = LimitsConfig {
        max_requests_per_second_per_token: 1,
        max_request_burst_per_token: 1,
        ..Default::default()
    };
    let app = apply_token_rate_limit(
        Router::new().route("/", post(|| async { StatusCode::INTERNAL_SERVER_ERROR })),
        &config,
    );

    let first = app
        .clone()
        .oneshot(request(Some("alice")))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let second = app.oneshot(request(Some("alice"))).await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 3: Run response and middleware tests and verify red**

Run:

```bash
cargo test -p rust-junosmcp-limits overload::tests::rate_limited_response_has_stable_contract_and_metric --locked
cargo test -p rust-junosmcp-limits rate_limit::tests::middleware_returns_exact_429_and_isolates_tokens --locked
```

Expected: FAIL because `rate_limited_response` and `apply_token_rate_limit` do not exist.

- [ ] **Step 4: Implement the dedicated response helper**

In `overload.rs`, import `CONTENT_TYPE` and add:

```rust
pub(crate) fn rate_limited_response(retry_after_secs: u64) -> Response {
    crate::prometheus::record_limit_hit("token_rate", "request_rejected");
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (RETRY_AFTER, retry_after_secs.to_string()),
            (CONTENT_TYPE, "application/json".to_owned()),
        ],
        r#"{"error":"rate_limited","limit":"token_rate"}"#,
    )
        .into_response()
}
```

Do not add `token_rate` to `overload_response`'s 503 allowlist; the dedicated
helper is the only path that records this new rejection.

- [ ] **Step 5: Implement the private middleware and public router helper**

Add the production imports and functions to `rate_limit.rs`:

```rust
use crate::overload::rate_limited_response;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;
use rust_junosmcp_auth::caller::CallerCtx;

async fn token_rate_limit_middleware(
    State(state): State<TokenRateLimitState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(caller) = request.extensions().get::<CallerCtx>() {
        let token = caller.token_name.clone();
        if let RateDecision::Limited { retry_after_secs } =
            state.check_at(&token, Instant::now())
        {
            tracing::warn!(
                limit = "token_rate",
                token = %token,
                rate = state.rate_per_second,
                burst = state.burst,
                retry_after_secs,
                "request rate limited"
            );
            return rate_limited_response(retry_after_secs);
        }
    }
    next.run(request).await
}

pub fn apply_token_rate_limit(router: Router, config: &LimitsConfig) -> Router {
    if !config.token_rate_limit_enabled() {
        return router;
    }
    router.layer(axum::middleware::from_fn_with_state(
        TokenRateLimitState::new(config),
        token_rate_limit_middleware,
    ))
}
```

Export only the helper from `lib.rs`:

```rust
pub use rate_limit::apply_token_rate_limit;
```

- [ ] **Step 6: Format and run all shared-limit tests**

Run:

```bash
cargo fmt --all
cargo test -p rust-junosmcp-limits --locked
cargo clippy -p rust-junosmcp-limits --all-targets -- -D warnings
```

Expected: the exact 429/metric, bucket, isolation, cancellation, and composition
tests pass; every existing 503 test remains green.

- [ ] **Step 7: Commit middleware and response behavior**

```bash
git add rust-junosmcp-limits/src/lib.rs rust-junosmcp-limits/src/overload.rs \
  rust-junosmcp-limits/src/rate_limit.rs
git commit -m "feat(150): enforce per-token HTTP request rate"
```

---

### Task 4: Both-Binary Wiring and Real HTTP Contracts

**Files:**
- Modify: `rust-junosmcp/src/http_transport.rs:10-94`
- Modify: `rust-srxmcp/src/http_transport.rs:10-133`
- Modify/Test: `rust-junosmcp/tests/http_limits.rs`
- Modify/Test: `rust-srxmcp/tests/http_limits.rs`

**Interfaces:**
- Consumes: public `apply_token_rate_limit` from Task 3 and both CLI/config fields from Task 1.
- Produces: identical Junos/SRX request flow `body -> auth -> rate -> concurrency/session -> rmcp` and real-process 429 contract evidence.

- [ ] **Step 1: Add the failing Junos endpoint contract test**

Append to `rust-junosmcp/tests/http_limits.rs`:

```rust
#[test]
fn per_token_rate_limit_returns_stable_429() {
    let inv = write_inv(
        r#"{"r1":{"ip":"203.0.113.1","port":1,"username":"u","auth":{"type":"password","password":"x"}}}"#,
    );
    let tokens = write_tokens(r#"{"version":1,"tokens":[]}"#);
    let alice = TokenStoreFile::add(
        tokens.path(),
        "alice",
        ScopeSet::Wildcard,
        ScopeSet::Wildcard,
    )
    .unwrap();
    let server = spawn_with_auth_args(
        inv.path(),
        tokens.path(),
        &[
            "--max-requests-per-second-per-token",
            "1",
            "--max-request-burst-per-token",
            "1",
        ],
    );

    let admitted = http_post(server.port, Some(alice.expose()), None, init_body());
    assert_eq!(admitted.code, 200);
    assert!(admitted.session_id.is_some());

    let limited = http_post(server.port, Some(alice.expose()), None, init_body());
    assert_eq!(limited.code, 429);
    assert_eq!(limited.retry_after.as_deref(), Some("1"));
    assert!(limited.session_id.is_none());
    assert_eq!(
        limited.body,
        serde_json::json!({"error": "rate_limited", "limit": "token_rate"})
    );
}
```

- [ ] **Step 2: Add the identical SRX endpoint contract test**

Append the same test to `rust-srxmcp/tests/http_limits.rs`. Its existing imports
already expose the same harness and auth types, so no endpoint-specific code is
needed.

- [ ] **Step 3: Run both endpoint tests and verify red**

Run:

```bash
cargo test -p rust-junosmcp --test http_limits per_token_rate_limit_returns_stable_429 --locked
cargo test -p rust-srxmcp --test http_limits per_token_rate_limit_returns_stable_429 --locked
```

Expected: FAIL because the second request is admitted (or otherwise reaches
rmcp) instead of returning 429; CLI parsing and config validation already work.

- [ ] **Step 4: Install the shared layer in both transports**

Add `apply_token_rate_limit` to each `rust_junosmcp_limits` import list. After
the existing concurrency layer and before the auth layer, add:

```rust
// Rate limiting wraps concurrency but remains inside auth, so CallerCtx exists
// and an over-rate request acquires no concurrency/session capacity.
let app = apply_token_rate_limit(app, &limits);
```

Update the adjacent comments to state that auth runs before rate/concurrency and
that the rate layer runs before concurrency in request order. Do not move the
metrics merge; `/metrics` must remain outside MCP auth and limits.

- [ ] **Step 5: Run endpoint parity and regression tests**

Run:

```bash
cargo fmt --all
cargo test -p rust-junosmcp --test http_limits --locked
cargo test -p rust-srxmcp --test http_limits --locked
cargo test -p rust-junosmcp --test http_metrics --locked
cargo test -p rust-srxmcp --test http_metrics --locked
cargo clippy -p rust-junosmcp -p rust-srxmcp --all-targets -- -D warnings
```

Expected: both new 429 tests pass, the existing 413/503 tests remain unchanged,
and `/metrics` remains unauthenticated and reachable only when enabled.

- [ ] **Step 6: Commit endpoint parity**

```bash
git add rust-junosmcp/src/http_transport.rs rust-srxmcp/src/http_transport.rs \
  rust-junosmcp/tests/http_limits.rs rust-srxmcp/tests/http_limits.rs
git commit -m "test(150): prove rate limiting on both HTTP endpoints"
```

---

### Task 5: Operator Documentation, Metrics Contract, and Changelogs

**Files:**
- Modify: `README.md:560-606`
- Modify: `docs/METRICS.md:53-82`
- Modify: `CHANGELOG.md:7-32`
- Modify: `rust-srxmcp/CHANGELOG.md:9-34`

**Interfaces:**
- Consumes: exact flags/env vars, response body, metric label, and layer semantics implemented in Tasks 1-4.
- Produces: operator-facing configuration and usage guidance plus release notes for both binaries.

- [ ] **Step 1: Update the README limit table and disabled-default wording**

Change the introduction to:

```markdown
Both endpoints enforce configurable DoS guardrails. Most limits are enabled by
default with generous values; the optional per-token request-rate limiter is
disabled until both of its knobs are positive. A zero value disables an
individual limit, subject to the rate/burst pair rule below.
```

Add these rows after per-token concurrency:

```markdown
| `--max-requests-per-second-per-token` | `JMCP_MAX_REQUESTS_PER_SECOND_PER_TOKEN` / `JMCP_SRX_MAX_REQUESTS_PER_SECOND_PER_TOKEN` | 0 | Per-token refill rate; pair with burst (`0/0` disables) |
| `--max-request-burst-per-token` | `JMCP_MAX_REQUEST_BURST_PER_TOKEN` / `JMCP_SRX_MAX_REQUEST_BURST_PER_TOKEN` | 0 | Per-token immediate burst; pair with rate (`0/0` disables) |
```

- [ ] **Step 2: Replace the deferred note with exact behavior and guidance**

Remove the #150 deferred line and add before the Prometheus paragraph:

```markdown
Per-token request-rate limiting is an opt-in token bucket keyed by the exact
authenticated token name. Set both the requests-per-second rate and burst to
positive values to enable it; leave both at `0` to disable it. Supplying only
one positive value fails startup. Each authenticated `/mcp` HTTP request costs
one token. An exhausted bucket returns **429**, `Retry-After: 1`, and
`{"error":"rate_limited","limit":"token_rate"}` before concurrency or session
capacity is acquired. Explicit no-auth mode skips this per-token control.

Use the RPS limiter to absorb bursts of many cheap, short calls. Use concurrency
limits to bound simultaneous expensive NETCONF/SSH work and slow response
streams. They are independent and can be enabled together; rate checks run
first, while concurrency/session exhaustion retains the existing **503**
contract.
```

- [ ] **Step 3: Extend the bounded metrics documentation**

Change the fixed `limit` values in `docs/METRICS.md` to:

```markdown
- limit: request_body, token_rate, global_concurrency, token_concurrency,
  router_concurrency, session_cap, or token_session_cap
```

Change the queue-time sentence to mention both immediate rejection classes:

```markdown
Queue time is not exported because request-rate and concurrency gates reject
immediately instead of queueing (`429` and `503`, respectively).
```

- [ ] **Step 4: Add matching Unreleased changelog entries**

Add under `### Added` in both changelogs:

```markdown
- **#150 - optional per-token request-rate limiting.** Both streamable-HTTP
  endpoints can enforce a continuously refilled token bucket for each exact
  authenticated token name using configurable whole-number RPS and burst
  knobs. The limiter is disabled by default; exhaustion returns stable `429`
  JSON with `Retry-After`, runs before existing concurrency/session gates, and
  exports the bounded `token_rate` limit metric without caller labels.
```

- [ ] **Step 5: Verify documentation consistency and stale-note removal**

Run:

```bash
rg -n "max-requests-per-second-per-token|max-request-burst-per-token|token_rate|rate_limited" README.md docs/METRICS.md CHANGELOG.md rust-srxmcp/CHANGELOG.md
! rg -n "Deferred.*#150|per-token RPS rate-limiting \(#150\)" README.md
git diff --check
```

Expected: both knobs appear in README, `token_rate` appears in metrics docs and
both changelogs, the deferred line is absent, and the diff has no whitespace
errors.

- [ ] **Step 6: Commit documentation**

```bash
git add README.md docs/METRICS.md CHANGELOG.md rust-srxmcp/CHANGELOG.md
git commit -m "docs(150): document per-token RPS limiting"
```

---

### Task 6: Full Verification, Review, and Branch Handoff

**Files:**
- Verify: all files changed since base commit `5bfd1f6cb27e9d101f0b4797ae6bc19683c0fe88`
- Verify unchanged: `Cargo.lock`, MCP schemas, token-file schema, generated files

**Interfaces:**
- Consumes: the complete implementation and documentation from Tasks 1-5.
- Produces: evidence suitable for independent review, PR creation, CI, merge, and cleanup.

- [ ] **Step 1: Audit the implementation against the spec and issue**

Run:

```bash
git diff --stat 5bfd1f6cb27e9d101f0b4797ae6bc19683c0fe88..HEAD
git diff --exit-code 5bfd1f6cb27e9d101f0b4797ae6bc19683c0fe88..HEAD -- Cargo.lock
rg -n "max_requests_per_second_per_token|max_request_burst_per_token|token_rate|TOO_MANY_REQUESTS" \
  rust-junosmcp-limits rust-junosmcp rust-srxmcp README.md docs/METRICS.md CHANGELOG.md rust-srxmcp/CHANGELOG.md
```

Expected: all five acceptance criteria map to implementation/tests/docs,
`Cargo.lock` has no diff, and no generated or secret-bearing file changed.

- [ ] **Step 2: Run repository-required formatting, lint, tests, and guard equivalents**

Run exactly:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Expected: all commands exit 0. Record total passed/ignored counts; ignored
real-device/network tests remain skipped.

- [ ] **Step 3: Run both offline CLI e2e help checks**

Run:

```bash
cargo run -p rust-junosmcp -- --help >/dev/null
cargo run -p rust-srxmcp -- --help >/dev/null
```

Expected: both exit 0 without contacting a device.

- [ ] **Step 4: Run the security gate and compare only against the accepted baseline**

Run:

```bash
/home/mharman/.local/share/mise/installs/trivy/0.70.0/trivy fs \
  --scanners vuln,misconfig,secret --exit-code 1 .
```

Expected accepted baseline only: `cmov 0.5.3` / CVE-2026-50185; Dockerfile
DS-0026 twice, DS-0002, and DS-0004; zero secrets. Any new finding blocks the
handoff.

- [ ] **Step 5: Repeat the release-check combination**

Because `just` is unavailable, rerun the exact underlying release recipes:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
/home/mharman/.local/share/mise/installs/trivy/0.70.0/trivy fs \
  --scanners vuln,misconfig,secret --exit-code 1 .
```

Expected: all code gates exit 0 and Trivy contains only the accepted baseline.

- [ ] **Step 6: Confirm branch cleanliness and intentional commits**

Run:

```bash
git status --short --branch
git log --oneline --decorate 5bfd1f6cb27e9d101f0b4797ae6bc19683c0fe88..HEAD
git diff --check 5bfd1f6cb27e9d101f0b4797ae6bc19683c0fe88..HEAD
```

Expected: clean branch, design/plan plus focused implementation/doc commits,
and no whitespace errors. If formatting or verification required a correction,
commit only that correction with a precise message and rerun its affected gate.

- [ ] **Step 7: Invoke independent code review before publishing**

Use `superpowers:requesting-code-review` against the full base-to-HEAD diff.
Address any Critical or Important finding with `superpowers:receiving-code-review`,
rerun affected tests, and request confirmation. Do not merge with an unresolved
Critical or Important finding.

- [ ] **Step 8: Publish, check CI, merge, and clean up**

Use the repository GitHub workflows in order:

1. `github:yeet` to push `issue-150-per-token-rate-limit` and open the PR with
   `Closes #150`, acceptance-criteria evidence, compatibility, skipped real-device
   tests, the accepted Trivy baseline, and the documented retained-historical-token-name
   map risk.
2. `github:gh-fix-ci` to inspect all GitHub Actions checks and logs; implement
   only evidence-backed fixes, push, and wait until every required check is green.
3. `superpowers:finishing-a-development-branch` to squash-merge after green CI.
4. Verify PR merged, issue #150 closed, the squash commit is on `origin/main`,
   and the merged feature tree matches the branch tree.
5. Remove the issue worktree, local branch, and remote branch; fast-forward the
   main checkout and prove it is clean and aligned with `origin/main`.

Expected: PR merged, issue #150 closed, no #150 worktree/branch remains, and
main is clean at the verified squash commit.
