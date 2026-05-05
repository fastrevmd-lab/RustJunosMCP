//! Per-request caller context populated by the auth middleware.

use rust_junosmcp_auth::{ScopeSet, TokenEntry};

// T10 will consume this in the #[tool] adapters; T11 wires it through the auth middleware.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CallerCtx {
    pub token_name: String,
    pub routers: ScopeSet,
    pub tools: ScopeSet,
}

impl From<&TokenEntry> for CallerCtx {
    fn from(e: &TokenEntry) -> Self {
        Self {
            token_name: e.name.clone(),
            routers: e.routers.clone(),
            tools: e.tools.clone(),
        }
    }
}
