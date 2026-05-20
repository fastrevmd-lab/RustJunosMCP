//! `JmcpSrxHandler` — rmcp `#[tool]` registry root for `rust-srxmcp`.
//! Phase 1A ships exactly one tool: `srxmcp_status`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Extensions, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::Instant;

#[derive(Clone)]
pub struct JmcpSrxHandler {
    started: Arc<Instant>,
}

impl JmcpSrxHandler {
    pub fn new(started: Arc<Instant>) -> Self {
        Self { started }
    }

    /// Pure tool body — used by the rmcp adapter below and by integration
    /// tests via `srxmcp_status_test`.
    fn srxmcp_status_body(&self, _args: SrxmcpStatusArgs) -> SrxmcpStatusResponse {
        let uptime_seconds = Instant::now()
            .saturating_duration_since(*self.started)
            .as_secs();
        SrxmcpStatusResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            endpoint: "srxmcp".to_string(),
            uptime_seconds,
        }
    }

    /// Test-only entry point so integration tests can drive the tool body
    /// without constructing an rmcp request envelope.
    pub fn srxmcp_status_test(&self, args: SrxmcpStatusArgs) -> SrxmcpStatusResponse {
        self.srxmcp_status_body(args)
    }
}

#[tool_router]
impl JmcpSrxHandler {
    #[tool(
        name = "srxmcp_status",
        description = "Diagnostic — returns this server's version, endpoint name, and uptime in seconds."
    )]
    async fn srxmcp_status(
        &self,
        Parameters(args): Parameters<SrxmcpStatusArgs>,
        _extensions: Extensions,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let resp = self.srxmcp_status_body(args);
        let body = serde_json::to_string_pretty(&resp).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("serializing SrxmcpStatusResponse: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for JmcpSrxHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "srxmcp-server".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: Some(
                "Juniper SRX-specific MCP server (Phase 1A scaffolding). \
                 Only `srxmcp_status` is wired in 0.0.1."
                    .into(),
            ),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SrxmcpStatusArgs {}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct SrxmcpStatusResponse {
    pub version: String,
    pub endpoint: String,
    pub uptime_seconds: u64,
}
