//! `get_junos_config` — return full text-format running config.

use crate::device_manager::DeviceManager;
use crate::error::JmcpError;
use crate::tools::GetConfigArgs;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle(args: GetConfigArgs, dm: Arc<DeviceManager>) -> Result<Value, JmcpError> {
    let mut dev = dm.open(&args.router_name).await?;
    let cfg_text = dev.cli("show configuration").await?;
    let _ = dev.close().await;
    Ok(json!(cfg_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Inventory;
    use std::io::Write;

    #[tokio::test]
    async fn unknown_router_propagates_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(
            br#"{
            "r1":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}
        }"#,
        )
        .unwrap();
        let inv = Arc::new(Inventory::load(f.path()).unwrap());
        let dm = Arc::new(DeviceManager::new(inv));
        let r = handle(
            GetConfigArgs {
                router_name: "nope".into(),
            },
            dm,
        )
        .await;
        assert!(matches!(r, Err(JmcpError::UnknownRouter(_))));
    }
}
