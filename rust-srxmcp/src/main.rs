//! `rust-srxmcp` — Phase 1A scaffolding. Task 10 wires HTTP transport.

use anyhow::Result;
use rust_srxmcp::server::JmcpSrxHandler;
use std::sync::Arc;
use tokio::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    let started = Arc::new(Instant::now());
    let _handler = JmcpSrxHandler::new(started);
    eprintln!(
        "rust-srxmcp {} — Task 9 stub (HTTP wire-up lands in Task 10)",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
