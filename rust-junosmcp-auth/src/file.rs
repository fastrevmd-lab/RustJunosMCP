//! On-disk token store: load, validate, atomic save.

use crate::store::{ScopeSet, TokenEntry, TokenStore};
use std::path::Path;

/// All v0.1 tool names. New sub-projects extend this list.
pub const KNOWN_TOOLS: &[&str] = &[
    "get_router_list",
    "gather_device_facts",
    "execute_junos_command",
    "get_junos_config",
    "junos_config_diff",
    "load_and_commit_config",
];

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("token store invalid: {0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OnDisk {
    version: u32,
    #[serde(default)]
    tokens: Vec<TokenEntry>,
}

pub struct TokenStoreFile;

impl TokenStoreFile {
    /// Load and validate. `known_routers` is from the current `devices.json`;
    /// unknown router names emit a `WARN` but keep the entry. Unknown tool
    /// names are fatal.
    pub fn load(path: &Path, known_routers: &[&str]) -> Result<TokenStore, TokenStoreError> {
        let bytes = std::fs::read(path)?;
        let parsed: OnDisk = serde_json::from_slice(&bytes)?;
        if parsed.version != 1 {
            return Err(TokenStoreError::Invalid(format!(
                "unsupported version: expected 1, got {}",
                parsed.version
            )));
        }

        // Validate each entry.
        for e in &parsed.tokens {
            if let ScopeSet::Allowlist(list) = &e.tools {
                for t in list {
                    if t == "*" {
                        return Err(TokenStoreError::Invalid(format!(
                            "token '{}' tools list mixes '*' with other names — \
                             use either [\"*\"] for wildcard or a list without '*'",
                            e.name
                        )));
                    }
                    if !KNOWN_TOOLS.contains(&t.as_str()) {
                        return Err(TokenStoreError::Invalid(format!(
                            "unknown tool name '{}' in token '{}': known tools are {:?}",
                            t, e.name, KNOWN_TOOLS
                        )));
                    }
                }
            }
            if let ScopeSet::Allowlist(list) = &e.routers {
                for r in list {
                    if r == "*" {
                        return Err(TokenStoreError::Invalid(format!(
                            "token '{}' routers list mixes '*' with other names — \
                             use either [\"*\"] for wildcard or a list without '*'",
                            e.name
                        )));
                    }
                }
                if !known_routers.is_empty() {
                    for r in list {
                        if !known_routers.iter().any(|kr| kr == r) {
                            tracing::warn!(token = %e.name, router = %r,
                                "token references router not present in current devices.json");
                        }
                    }
                }
            }
            if e.routers.is_empty_allowlist() {
                tracing::warn!(token = %e.name, "token routers scope is empty — token cannot reach any router");
            }
            if e.tools.is_empty_allowlist() {
                tracing::warn!(token = %e.name, "token tools scope is empty — token cannot call any tool");
            }
        }

        TokenStore::try_new(parsed.tokens).map_err(|e| TokenStoreError::Invalid(format!("duplicate: {}", e.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(json: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f
    }

    #[test]
    fn loads_minimal_valid_file() {
        let f = write_tmp(r#"{"version":1,"tokens":[]}"#);
        let store = TokenStoreFile::load(f.path(), &[]).unwrap();
        assert_eq!(store.len(), 0);
    }

    // Two distinct valid base64url-unpadded SHA-256 hashes (43 chars each).
    // base64ct enforces the trailing-bits-zero rule, so the last char must be
    // one of A/E/I/M/Q/U/Y/c/g/k/o/s/w/0/4/8 (bottom 2 bits zero).
    const HASH_A: &str = "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const HASH_B: &str = "sha256:EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE";

    #[test]
    fn loads_one_token() {
        let f = write_tmp(&format!(r#"{{
            "version":1,
            "tokens":[{{
                "name":"a",
                "hash":"{HASH_A}",
                "routers":["*"],
                "tools":["*"],
                "created_at":"2026-05-05T00:00:00Z"
            }}]
        }}"#));
        let store = TokenStoreFile::load(f.path(), &[]).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.entries()[0].name, "a");
    }

    #[test]
    fn rejects_wrong_version() {
        let f = write_tmp(r#"{"version":2,"tokens":[]}"#);
        let err = TokenStoreFile::load(f.path(), &[]).unwrap_err();
        assert!(matches!(err, TokenStoreError::Invalid(s) if s.contains("version")));
    }

    #[test]
    fn rejects_duplicate_names() {
        let f = write_tmp(&format!(r#"{{
            "version":1,
            "tokens":[
                {{"name":"a","hash":"{HASH_A}","routers":["*"],"tools":["*"],"created_at":"2026-05-05T00:00:00Z"}},
                {{"name":"a","hash":"{HASH_B}","routers":["*"],"tools":["*"],"created_at":"2026-05-05T00:00:00Z"}}
            ]
        }}"#));
        let err = TokenStoreFile::load(f.path(), &[]).unwrap_err();
        assert!(matches!(err, TokenStoreError::Invalid(s) if s.contains("duplicate")));
    }

    #[test]
    fn rejects_unknown_tool_name() {
        let f = write_tmp(&format!(r#"{{
            "version":1,
            "tokens":[{{
                "name":"a","hash":"{HASH_A}",
                "routers":["*"],"tools":["does_not_exist"],
                "created_at":"2026-05-05T00:00:00Z"
            }}]
        }}"#));
        let err = TokenStoreFile::load(f.path(), &[]).unwrap_err();
        assert!(matches!(err, TokenStoreError::Invalid(s) if s.contains("does_not_exist")));
    }

    #[test]
    fn rejects_malformed_hash() {
        let f = write_tmp(r#"{
            "version":1,
            "tokens":[{
                "name":"a","hash":"plaintext-bad",
                "routers":["*"],"tools":["*"],
                "created_at":"2026-05-05T00:00:00Z"
            }]
        }"#);
        let err = TokenStoreFile::load(f.path(), &[]).unwrap_err();
        // Serde returns a Json error here because TokenHash deserialization fails.
        assert!(matches!(err, TokenStoreError::Json(_)));
    }

    #[test]
    fn rejects_wildcard_mixed_into_allowlist() {
        // "*" inside an allowlist is ambiguous (would never act as wildcard
        // since ScopeSet::From<Vec<String>> only treats single-element ["*"]
        // as Wildcard). Make this fatal at load to keep one canonical spelling.
        let f = write_tmp(&format!(r#"{{
            "version":1,
            "tokens":[{{
                "name":"a","hash":"{HASH_A}",
                "routers":["*","r1"],"tools":["*"],
                "created_at":"2026-05-05T00:00:00Z"
            }}]
        }}"#));
        let err = TokenStoreFile::load(f.path(), &[]).unwrap_err();
        assert!(matches!(err, TokenStoreError::Invalid(s) if s.contains("'*'")));
    }

    #[test]
    fn warns_but_keeps_unknown_router_name() {
        // unknown_routers: known_routers passed in is &[]; the entry references
        // "r1" which is not in that list. Load should still succeed.
        let f = write_tmp(&format!(r#"{{
            "version":1,
            "tokens":[{{
                "name":"a","hash":"{HASH_A}",
                "routers":["r1"],"tools":["*"],
                "created_at":"2026-05-05T00:00:00Z"
            }}]
        }}"#));
        let store = TokenStoreFile::load(f.path(), &[]).unwrap();
        assert_eq!(store.len(), 1);
    }
}
