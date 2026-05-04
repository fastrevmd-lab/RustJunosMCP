//! MCP tool argument types. Each tool gets a typed input struct that
//! `schemars` derives a JSON schema from for advertisement to the client.

use schemars::JsonSchema;
use serde::Deserialize;

pub mod config_diff;
pub mod execute_command;
pub mod facts;
pub mod get_config;
pub mod load_commit;
pub mod router_list;

fn default_timeout() -> u64 {
    360
}
fn default_version() -> i64 {
    1
}
fn default_set_format() -> String {
    "set".into()
}
fn default_commit_comment() -> String {
    "Configuration loaded via MCP".into()
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct EmptyArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteCommandArgs {
    /// The name of the router.
    pub router_name: String,
    /// The command to execute on the router.
    pub command: String,
    /// Command timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetConfigArgs {
    pub router_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfigDiffArgs {
    pub router_name: String,
    /// Rollback version to compare against (1-49).
    #[serde(default = "default_version")]
    pub version: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GatherFactsArgs {
    pub router_name: String,
    /// Connection timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LoadCommitArgs {
    pub router_name: String,
    /// The configuration text to load.
    pub config_text: String,
    /// Format: set, text, or xml.
    #[serde(default = "default_set_format")]
    pub config_format: String,
    /// Commit comment recorded in the device commit log.
    #[serde(default = "default_commit_comment")]
    pub commit_comment: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_command_defaults_timeout() {
        let v = serde_json::json!({"router_name":"r1","command":"show version"});
        let a: ExecuteCommandArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.timeout, 360);
    }

    #[test]
    fn config_diff_defaults_version_to_1() {
        let v = serde_json::json!({"router_name":"r1"});
        let a: ConfigDiffArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.version, 1);
    }

    #[test]
    fn load_commit_defaults_format_and_comment() {
        let v = serde_json::json!({"router_name":"r1","config_text":"set x"});
        let a: LoadCommitArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.config_format, "set");
        assert_eq!(a.commit_comment, "Configuration loaded via MCP");
    }

    #[test]
    fn execute_command_rejects_missing_required() {
        let v = serde_json::json!({"router_name":"r1"});
        let r: Result<ExecuteCommandArgs, _> = serde_json::from_value(v);
        assert!(r.is_err());
    }
}
