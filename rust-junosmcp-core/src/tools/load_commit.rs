//! `load_and_commit_config` — lock candidate, load, diff, commit (with comment),
//! unlock. Rollback on commit failure. Returns `{success, diff, error?}`.
//!
//! Commit comment is sent via raw RPC because `rustez::ConfigManager` does not
//! yet expose `commit_with_comment` (followup #2).

use crate::device_manager::DeviceManager;
use crate::error::JmcpError;
use crate::helpers::build_config_payload;
use crate::tools::LoadCommitArgs;
use serde_json::{json, Value};
use std::sync::Arc;

/// XML-escape the commit comment for safe inclusion in the raw RPC body.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('\'', "&apos;")
     .replace('"', "&quot;")
}

pub async fn handle(
    args: LoadCommitArgs,
    dm: Arc<DeviceManager>,
) -> Result<Value, JmcpError> {
    let payload = build_config_payload(args.config_text, Some(&args.config_format))?;
    let comment_xml = format!(
        "<commit><log>{}</log></commit>",
        xml_escape(&args.commit_comment),
    );

    let mut dev = dm.open(&args.router_name).await?;
    let mut cfg = dev.config()?;

    cfg.lock().await?;
    if let Err(e) = cfg.load(payload).await {
        let _ = cfg.unlock().await;
        let _ = dev.close().await;
        return Err(JmcpError::Rustez(e));
    }
    let diff = cfg.diff().await?.unwrap_or_default();

    // Commit with comment via raw RPC (followup #2).
    let commit_result = dev.rpc()?.call_xml(&comment_xml).await;

    let result = match commit_result {
        Ok(_) => json!({ "success": true, "diff": diff }),
        Err(e) => {
            // Discard the candidate so the next session starts clean.
            // rollback(0) discards uncommitted changes.
            if let Ok(mut cfg2) = dev.config() {
                let _ = cfg2.rollback(0).await;
                let _ = cfg2.unlock().await;
            }
            json!({ "success": false, "diff": diff, "error": e.to_string() })
        }
    };

    // Best-effort unlock + close.
    if let Ok(mut cfg2) = dev.config() {
        let _ = cfg2.unlock().await;
    }
    let _ = dev.close().await;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Inventory;
    use std::io::Write;

    #[test]
    fn xml_escape_handles_specials() {
        assert_eq!(xml_escape("a & <b> 'c' \"d\""),
                   "a &amp; &lt;b&gt; &apos;c&apos; &quot;d&quot;");
    }

    #[tokio::test]
    async fn unknown_router_propagates_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(br#"{
            "r1":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}
        }"#).unwrap();
        let inv = Arc::new(Inventory::load(f.path()).unwrap());
        let dm  = Arc::new(DeviceManager::new(inv));
        let r = handle(
            LoadCommitArgs {
                router_name: "nope".into(),
                config_text: "set system foo".into(),
                config_format: "set".into(),
                commit_comment: "test".into(),
            },
            dm,
        ).await;
        assert!(matches!(r, Err(JmcpError::UnknownRouter(_))));
    }

    #[tokio::test]
    async fn invalid_format_rejected_before_connect() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(br#"{
            "r1":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}
        }"#).unwrap();
        let inv = Arc::new(Inventory::load(f.path()).unwrap());
        let dm  = Arc::new(DeviceManager::new(inv));
        let r = handle(
            LoadCommitArgs {
                router_name: "r1".into(),
                config_text: "x".into(),
                config_format: "yaml".into(),
                commit_comment: "test".into(),
            },
            dm,
        ).await;
        assert!(matches!(r, Err(JmcpError::BadFormat(ref s)) if s == "yaml"));
    }
}
