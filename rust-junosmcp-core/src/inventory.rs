//! `devices.json` parsing and validation.
//!
//! Drop-in compatible with Juniper/junos-mcp-server.

use serde::Deserialize;
use std::path::PathBuf;

/// Authentication config for a Junos device. Tagged enum mirrors the Python
/// repo's `auth.type` discriminator.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    Password { password: String },
    SshKey { private_key_path: PathBuf },
}

// Hand-written Debug to redact passwords. Never derive Debug on this enum.
impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password { .. } => f.debug_struct("Password")
                .field("password", &"<redacted>")
                .finish(),
            Self::SshKey { private_key_path } => f.debug_struct("SshKey")
                .field("private_key_path", private_key_path)
                .finish(),
        }
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    #[test]
    fn password_debug_does_not_leak_secret() {
        let auth = AuthConfig::Password { password: "hunter2".into() };
        let s = format!("{auth:?}");
        assert!(!s.contains("hunter2"), "debug output leaked the password: {s}");
        assert!(s.contains("redacted"));
    }

    #[test]
    fn ssh_key_debug_shows_path() {
        let auth = AuthConfig::SshKey { private_key_path: "/tmp/k.pem".into() };
        let s = format!("{auth:?}");
        assert!(s.contains("/tmp/k.pem"));
    }

    #[test]
    fn deserialize_password() {
        let json = r#"{"type":"password","password":"x"}"#;
        let parsed: AuthConfig = serde_json::from_str(json).unwrap();
        match parsed {
            AuthConfig::Password { password } => assert_eq!(password, "x"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserialize_ssh_key() {
        let json = r#"{"type":"ssh_key","private_key_path":"/k.pem"}"#;
        let parsed: AuthConfig = serde_json::from_str(json).unwrap();
        match parsed {
            AuthConfig::SshKey { private_key_path } =>
                assert_eq!(private_key_path, std::path::PathBuf::from("/k.pem")),
            _ => panic!("wrong variant"),
        }
    }
}

fn default_port() -> u16 { 22 }

/// One entry in `devices.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct DeviceEntry {
    pub ip: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    pub auth: AuthConfig,
    /// Optional path to OpenSSH config file (jumphost). Parsed but not yet
    /// honored — see [`crate::error::JmcpError::SshConfigUnsupported`].
    #[serde(default)]
    pub ssh_config: Option<PathBuf>,
}

#[cfg(test)]
mod entry_tests {
    use super::*;

    #[test]
    fn parses_password_entry_with_default_port() {
        let json = r#"{
            "ip":"10.0.0.1",
            "username":"admin",
            "auth":{"type":"password","password":"x"}
        }"#;
        let e: DeviceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.ip, "10.0.0.1");
        assert_eq!(e.port, 22);
        assert_eq!(e.username, "admin");
        assert!(e.ssh_config.is_none());
    }

    #[test]
    fn parses_ssh_key_entry_with_explicit_port_and_ssh_config() {
        let json = r#"{
            "ip":"10.0.0.2",
            "port":830,
            "username":"netconf",
            "ssh_config":"/home/u/.ssh/config_jh",
            "auth":{"type":"ssh_key","private_key_path":"/k.pem"}
        }"#;
        let e: DeviceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.port, 830);
        assert_eq!(e.ssh_config, Some(PathBuf::from("/home/u/.ssh/config_jh")));
    }

    #[test]
    fn rejects_missing_required_fields() {
        let json = r#"{"username":"admin","auth":{"type":"password","password":"x"}}"#;
        let r: Result<DeviceEntry, _> = serde_json::from_str(json);
        assert!(r.is_err(), "expected error for missing 'ip'");
    }
}
