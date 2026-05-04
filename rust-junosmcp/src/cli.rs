//! Command-line arguments. v0.1 only supports stdio transport. The
//! `streamable-http` value is parsed but rejected at runtime so the user
//! sees a clear error instead of silent fallback.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Parser)]
#[command(name = "rust-junosmcp", version, about = "Junos MCP server (Rust)")]
pub struct Cli {
    /// JSON file with device mapping (Juniper junos-mcp-server compatible).
    #[arg(short = 'f', long, default_value = "devices.json")]
    pub device_mapping: PathBuf,

    /// Transport. v0.1 only supports stdio.
    #[arg(short = 't', long, default_value = "stdio", value_enum)]
    pub transport: Transport,

    /// Bind host (accepted for forward-compat; only used when streamable-http lands in v0.2).
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    pub host: String,

    /// Bind port (accepted for forward-compat; only used when streamable-http lands in v0.2).
    #[arg(short = 'p', long, default_value_t = 30030)]
    pub port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn defaults() {
        let cli = Cli::parse_from(["rust-junosmcp"]);
        assert_eq!(cli.device_mapping, PathBuf::from("devices.json"));
        assert_eq!(cli.transport, Transport::Stdio);
        assert_eq!(cli.host, "127.0.0.1");
        assert_eq!(cli.port, 30030);
    }

    #[test]
    fn parses_short_flags() {
        let cli = Cli::parse_from(["rust-junosmcp", "-f", "/etc/jmcp/d.json"]);
        assert_eq!(cli.device_mapping, PathBuf::from("/etc/jmcp/d.json"));
    }

    #[test]
    fn parses_streamable_http_value() {
        let cli = Cli::parse_from(["rust-junosmcp", "-t", "streamable-http"]);
        assert_eq!(cli.transport, Transport::StreamableHttp);
    }
}
