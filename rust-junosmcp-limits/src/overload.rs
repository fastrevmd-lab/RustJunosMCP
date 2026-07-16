//! Stable overload responses: HTTP 503 + `Retry-After`, load-shed semantics.

use axum::http::{header::RETRY_AFTER, StatusCode};
use axum::response::{IntoResponse, Response};

/// Seconds advertised in `Retry-After` on every shed response.
const RETRY_AFTER_SECS: u64 = 1;

/// Build a stable overload response for the given limit kind
/// (e.g. `"global_concurrency"`, `"token_concurrency"`, `"session_cap"`).
pub fn overload_response(limit_kind: &'static str) -> Response {
    crate::prometheus::record_limit_hit(limit_kind, "request_rejected");
    let body = format!(r#"{{"error":"overloaded","limit":"{limit_kind}"}}"#);
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(RETRY_AFTER, RETRY_AFTER_SECS.to_string())],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overload_response_counts_each_fixed_limit_kind() {
        let (recorder, handle) = crate::prometheus::test_recorder("junos");
        metrics::with_local_recorder(&recorder, || {
            for limit in [
                "global_concurrency",
                "token_concurrency",
                "router_concurrency",
                "session_cap",
                "token_session_cap",
            ] {
                let _ = overload_response(limit);
            }
        });
        handle.run_upkeep();
        let text = handle.render();
        for limit in [
            "global_concurrency",
            "token_concurrency",
            "router_concurrency",
            "session_cap",
            "token_session_cap",
        ] {
            assert!(
                text.lines().any(|line| {
                    line.starts_with("junosmcp_limit_hits_total{")
                        && line.contains(&format!("limit=\"{limit}\""))
                        && line.contains("event=\"request_rejected\"")
                        && line.ends_with(" 1")
                }),
                "missing {limit} in:\n{text}"
            );
        }
    }
}
