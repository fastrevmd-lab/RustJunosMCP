//! `add_device` — validate, persist atomically, swap inventory.

use crate::device_manager::DeviceManager;
use crate::error::JmcpError;
use crate::inventory::AuthConfig;
use crate::tools::AddDeviceArgs;

/// Resolved + validated argument bundle. Produced by `validate()`.
#[derive(Debug)]
pub struct ResolvedAdd {
    pub device_name: String,
    pub device_ip: String,
    pub device_port: u32,
    pub username: String,
    pub auth: AuthConfig,
}

/// Pure validation: returns the resolved bundle or the most specific error.
/// Does NOT touch disk or the device manager's locks.
pub fn validate(args: &AddDeviceArgs, dm: &DeviceManager) -> Result<ResolvedAdd, JmcpError> {
    if dm.inventory_readonly() {
        return Err(JmcpError::InventoryReadonly);
    }

    let mut missing: Vec<String> = Vec::new();
    if args.device_name.is_none() {
        missing.push("device_name".into());
    }
    if args.device_ip.is_none() {
        missing.push("device_ip".into());
    }
    if args.username.is_none() {
        missing.push("username".into());
    }
    if args.auth.is_none() {
        missing.push("auth".into());
    }
    if !missing.is_empty() {
        return Err(JmcpError::MissingArguments(missing));
    }

    let device_name = args.device_name.clone().unwrap();
    if !is_valid_device_name(&device_name) {
        return Err(JmcpError::InvalidDeviceName(device_name));
    }
    let inv = dm.inventory();
    if inv.get(&device_name).is_ok() {
        return Err(JmcpError::DeviceExists(device_name));
    }

    let device_ip = args.device_ip.clone().unwrap();
    if !is_valid_ip_or_hostname(&device_ip) {
        return Err(JmcpError::InvalidDeviceIp(device_ip));
    }

    let device_port = args.device_port.unwrap_or(22);
    if !(1..=65535).contains(&device_port) {
        return Err(JmcpError::InvalidDevicePort(device_port));
    }

    let auth = args.auth.clone().unwrap();
    if matches!(auth, AuthConfig::Password { .. }) && !dm.allow_password_auth_add() {
        return Err(JmcpError::PasswordAuthDisabled);
    }

    let username = args.username.clone().unwrap();

    Ok(ResolvedAdd {
        device_name,
        device_ip,
        device_port,
        username,
        auth,
    })
}

fn is_valid_device_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn is_valid_ip_or_hostname(s: &str) -> bool {
    if s.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    // RFC 1123 hostname: 1..=253 chars, labels split on '.', each label
    // 1..=63 chars matching [A-Za-z0-9-] without leading/trailing hyphen.
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    s.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Inventory;
    use std::io::Write;
    use std::sync::Arc;

    fn dm_with(json: &str, readonly: bool, allow_pw: bool) -> Arc<DeviceManager> {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        let inv = Arc::new(Inventory::load(f.path()).unwrap());
        Arc::new(DeviceManager::with_path(
            inv,
            f.path().to_path_buf(),
            crate::inventory::hash_file(f.path()).unwrap(),
            readonly,
            allow_pw,
        ))
    }

    fn args_full() -> AddDeviceArgs {
        AddDeviceArgs {
            device_name: Some("core-3".into()),
            device_ip: Some("10.0.0.3".into()),
            device_port: Some(22),
            username: Some("automation".into()),
            auth: Some(AuthConfig::SshKey {
                private_key_path: "/etc/jmcp/keys/id".into(),
            }),
        }
    }

    #[test]
    fn rejects_when_inventory_readonly() {
        let dm = dm_with(r#"{}"#, true, false);
        let r = validate(&args_full(), &dm);
        assert!(matches!(r, Err(JmcpError::InventoryReadonly)));
    }

    #[test]
    fn rejects_existing_device_name() {
        let dm = dm_with(
            r#"{"core-3":{"ip":"127.0.0.1","username":"u","auth":{"type":"password","password":"x"}}}"#,
            false,
            true,
        );
        let r = validate(&args_full(), &dm);
        assert!(matches!(r, Err(JmcpError::DeviceExists(ref n)) if n == "core-3"));
    }

    #[test]
    fn rejects_missing_required_fields_with_list() {
        let dm = dm_with(r#"{}"#, false, false);
        let mut a = args_full();
        a.device_name = None;
        a.username = None;
        let r = validate(&a, &dm);
        match r {
            Err(JmcpError::MissingArguments(v)) => {
                assert!(v.contains(&"device_name".to_string()));
                assert!(v.contains(&"username".to_string()));
            }
            other => panic!("expected MissingArguments, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_name_with_shell_meta() {
        let dm = dm_with(r#"{}"#, false, false);
        let mut a = args_full();
        a.device_name = Some("evil; rm -rf /".into());
        let r = validate(&a, &dm);
        assert!(matches!(r, Err(JmcpError::InvalidDeviceName(_))));
    }

    #[test]
    fn rejects_invalid_ip_garbage() {
        let dm = dm_with(r#"{}"#, false, false);
        let mut a = args_full();
        a.device_ip = Some("not an ip or host".into());
        let r = validate(&a, &dm);
        assert!(matches!(r, Err(JmcpError::InvalidDeviceIp(_))));
    }

    #[test]
    fn accepts_hostname_form() {
        let dm = dm_with(r#"{}"#, false, false);
        let mut a = args_full();
        a.device_ip = Some("router-3.example.net".into());
        let r = validate(&a, &dm).unwrap();
        assert_eq!(r.device_ip, "router-3.example.net");
    }

    #[test]
    fn rejects_out_of_range_port() {
        let dm = dm_with(r#"{}"#, false, false);
        let mut a = args_full();
        a.device_port = Some(70_000);
        let r = validate(&a, &dm);
        assert!(matches!(r, Err(JmcpError::InvalidDevicePort(70_000))));
    }

    #[test]
    fn rejects_password_auth_when_flag_disabled() {
        let dm = dm_with(r#"{}"#, false, false);
        let mut a = args_full();
        a.auth = Some(AuthConfig::Password {
            password: "x".into(),
        });
        let r = validate(&a, &dm);
        assert!(matches!(r, Err(JmcpError::PasswordAuthDisabled)));
    }

    #[test]
    fn accepts_password_auth_when_flag_enabled() {
        let dm = dm_with(r#"{}"#, false, true);
        let mut a = args_full();
        a.auth = Some(AuthConfig::Password {
            password: "x".into(),
        });
        validate(&a, &dm).unwrap();
    }
}
