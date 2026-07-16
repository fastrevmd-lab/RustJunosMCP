//! HTTP resource, concurrency, and session limits for the streamable-HTTP
//! endpoints shared by `rust-junosmcp` and `rust-srxmcp`.

mod concurrency;
mod config;
mod overload;
mod prometheus;
// Removed in Task 3 when the middleware consumes the rate-limit core.
#[cfg_attr(not(test), allow(dead_code))]
mod rate_limit;
mod router;
mod session;

pub use concurrency::{apply_body_limit, concurrency_middleware, ConcurrencyState};
pub use config::{LimitsConfig, LimitsConfigError};
pub use overload::overload_response;
pub use prometheus::PrometheusRuntime;
pub use session::{LimitedSessionManager, LimitedSessionManagerError, SessionTracker};
