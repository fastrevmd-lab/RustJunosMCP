//! `manage_idp_security_package` — IDP signature-package lifecycle.
//!
//! Scope this file ships **today (Task 4, v0.2.0 milestone 1)**:
//! * Full args / action surface so callers can wire the MCP tool now.
//! * The `check_server` verb end-to-end (read-only, single-call,
//!   not audited — see design doc §"Two-call confirmation protocol").
//! * Pure parsers for the two RPCs `check_server` needs.
//!
//! Out of scope for Task 4 (lands in Tasks 5+):
//! * `download_and_install` verb (call 1 plan emission + call 2 destructive path).
//! * `rollback` verb.
//! * The pre-flight device-touching wrappers (`license_active`,
//!   `cluster_topology`, `signatures_server_reachable`).
//!
//! # Live-captured RPC contract (see design Appendix A)
//!
//! * `get-idp-security-package-information` →
//!   `<idp-security-package-information>` (standalone) or
//!   `<multi-routing-engine-results>` wrapping one per node.
//!   `<security-package-version>` carries the full text (e.g.
//!   `"3910(Minor, Thu May 21 …)"`) or `"N/A(N/A)"` on fresh devices.
//! * `request-idp-security-package-check-server` →
//!   `<secpack-download-status>` with free-text
//!   `<secpack-download-status-detail>`. The version is regex-extracted
//!   from `Version info:NNNN(...)`. If the configured signature URL
//!   is unreachable, the reply is `<xnm:error>` with message
//!   `"Fetching signed manifest.xml failed, error: Server not reachable"`.

use crate::workflows::signature_package::Service;
use crate::SrxError;
use rust_junosmcp_core::device_manager::PooledDevice;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── RPC names (live-verified, see design Appendix A) ──────────────────────────
//
// Module constants so a future Junos rename only edits one place per RPC.

const RPC_PACKAGE_INFORMATION: &str = "get-idp-security-package-information";
const RPC_CHECK_SERVER: &str = "request-idp-security-package-download-check-server";

// ── Public arg surface ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdpAction {
    CheckServer,
    DownloadAndInstall,
    Rollback,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IdpPackageArgs {
    pub router: String,
    pub action: IdpAction,
    /// Pin to a specific package version (e.g. `"3714"`). Only meaningful
    /// for `download_and_install`; ignored otherwise.
    #[serde(default)]
    pub version: Option<String>,
    /// Required for destructive actions (`download_and_install`, `rollback`).
    /// Ignored for `check_server`.
    #[serde(default)]
    pub confirm: bool,
    /// Per-call outer budget in seconds (download poll + install poll combined).
    /// Default 600s (10 min), cap 1800s (30 min).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Append raw RPC replies to the response for debugging.
    #[serde(default)]
    pub include_raw: bool,
}

// ── `check_server` response types ─────────────────────────────────────────────

/// One row of the `nodes` array on the `check_server` response.
///
/// `re_name` is `""` for standalone devices, `"node0"` / `"node1"` for clusters.
/// `current_package_version` is the raw `<security-package-version>` text from
/// the device — `None` only when the element is missing or its text is
/// `"N/A(N/A)"` (fresh device with no signatures ever installed).
#[derive(Debug, Serialize, JsonSchema, Clone, PartialEq, Eq)]
pub struct IdpCheckServerNode {
    pub re_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_package_version: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema, Clone, PartialEq, Eq)]
pub struct IdpCheckServerData {
    pub router: String,
    pub service: Service,
    pub topology: crate::workflows::signature_package::Topology,
    /// Leading numeric version reported by the Juniper signatures server
    /// (e.g. `"3910"`). Pulled from the `Version info:NNNN(...)` line in
    /// the `<secpack-download-status-detail>` free text.
    pub latest_version: String,
    pub nodes: Vec<IdpCheckServerNode>,
    /// True iff any node's `current_package_version` leading numeric does
    /// not match `latest_version`. A fresh device (`current = None`) counts
    /// as "needs update".
    pub update_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_xml: Option<String>,
}

// ── `check_server` — async entry point ────────────────────────────────────────

/// Run the read-only `check_server` verb. Issues two RPCs back-to-back:
/// 1. `get-idp-security-package-information` for the current installed version(s)
/// 2. `request-idp-security-package-download-check-server` for the latest
///    version published by `signatures.juniper.net`.
pub async fn check_server(
    device: &mut PooledDevice,
    args: &IdpPackageArgs,
) -> Result<IdpCheckServerData, SrxError> {
    if args.router.trim().is_empty() {
        return Err(SrxError::InvalidInput("router must not be empty".into()));
    }
    let mut exec = device
        .rpc()
        .map_err(|e| SrxError::Transport(rust_junosmcp_core::JmcpError::from(e)))?;

    let info_xml = exec
        .call(RPC_PACKAGE_INFORMATION, &[])
        .await
        .map_err(|e| SrxError::Transport(rust_junosmcp_core::JmcpError::from(e)))?;
    let check_xml = exec
        .call(RPC_CHECK_SERVER, &[])
        .await
        .map_err(|e| SrxError::Transport(rust_junosmcp_core::JmcpError::from(e)))?;

    let nodes = parse_package_information(&info_xml)?;
    let latest_version = parse_check_server_reply(&check_xml, &args.router)?;

    let topology = if nodes.len() > 1 {
        crate::workflows::signature_package::Topology::ChassisCluster
    } else {
        crate::workflows::signature_package::Topology::Standalone
    };

    let update_available = nodes.iter().any(|n| {
        match n.current_package_version.as_deref() {
            None => true, // fresh device — always upgradeable
            Some(v) => leading_version_number(v) != leading_version_number(&latest_version),
        }
    });

    let raw_xml = if args.include_raw {
        Some(format!(
            "<!-- package-information -->\n{info_xml}\n<!-- check-server -->\n{check_xml}"
        ))
    } else {
        None
    };

    Ok(IdpCheckServerData {
        router: args.router.clone(),
        service: Service::Idp,
        topology,
        latest_version,
        nodes,
        update_available,
        raw_xml,
    })
}

// ── Parsers (pure, unit-testable) ─────────────────────────────────────────────

/// Parse a `<idp-security-package-information>` reply (standalone) or a
/// `<multi-routing-engine-results>` envelope wrapping one
/// `<idp-security-package-information>` per node (cluster).
///
/// Returns one [`IdpCheckServerNode`] per RE. `current_package_version` is
/// `None` when the device reports `"N/A(N/A)"` (fresh device, no signatures
/// ever installed) or the element is absent.
pub fn parse_package_information(reply_xml: &str) -> Result<Vec<IdpCheckServerNode>, SrxError> {
    let split = crate::xml::multi_re_split(reply_xml)?;
    if split.is_empty() {
        return Err(SrxError::schema_mismatch(
            RPC_PACKAGE_INFORMATION,
            "multi-routing-engine-item",
        ));
    }

    let mut out = Vec::with_capacity(split.len());
    for node in split {
        let info_xml = &node.inner_xml;
        // Standalone replies already start with <idp-security-package-information>;
        // for multi-RE, inner_xml contains that element directly too.
        let version_text = crate::xml::text_of(info_xml, "security-package-version");
        let normalized = version_text.and_then(|v| normalize_version_text(&v));
        out.push(IdpCheckServerNode {
            re_name: node.re_name,
            current_package_version: normalized,
        });
    }
    Ok(out)
}

/// Extract the latest-version string from a `check-server` reply.
///
/// Happy-path reply shape:
/// ```xml
/// <secpack-download-status format="xml">
///   <secpack-download-status-detail>Successfully retrieved from(https://signatures.juniper.net/cgi-bin/index.cgi).
/// Version info:3910(Minor, Detector=12.6.180250827, Templates=3910)</secpack-download-status-detail>
/// </secpack-download-status>
/// ```
///
/// Returns `"3910"`.
///
/// If the reply is an `<xnm:error>` with `"Server not reachable"` in the
/// message text, returns [`SrxError::SignaturePackageServerUnreachable`].
pub fn parse_check_server_reply(reply_xml: &str, router: &str) -> Result<String, SrxError> {
    // xnm:error channel first (see design Appendix A.2).
    if reply_xml.contains("<xnm:error") || reply_xml.contains("xmlns:xnm") {
        let msg = crate::xml::text_of(reply_xml, "message").unwrap_or_default();
        if !msg.is_empty() {
            return Err(SrxError::SignaturePackageServerUnreachable {
                router: router.to_string(),
                detail: msg,
            });
        }
    }

    let detail =
        crate::xml::text_of(reply_xml, "secpack-download-status-detail").ok_or_else(|| {
            SrxError::schema_mismatch(RPC_CHECK_SERVER, "secpack-download-status-detail")
        })?;

    // In-band "Done;...Failed;..." channel (rare on check-server but possible
    // — Junos uses the literal "Failed;" token per design Appendix A.2).
    if detail.contains("Failed;") {
        return Err(SrxError::SignaturePackageServerUnreachable {
            router: router.to_string(),
            detail,
        });
    }

    // Regex out "Version info:NNNN".
    extract_version_info(&detail).ok_or_else(|| {
        SrxError::Parse(format!(
            "{RPC_CHECK_SERVER}: missing 'Version info:NNNN' in detail text: {detail:?}"
        ))
    })
}

/// Normalise a `<security-package-version>` text:
/// * `"N/A(N/A)"` / `"N/A"` / empty / whitespace → `None`.
/// * Anything else → `Some(trimmed)`.
fn normalize_version_text(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("n/a") || t.starts_with("N/A(") {
        return None;
    }
    Some(t.to_string())
}

/// Extract the leading numeric token from a `Version info:NNNN(...)` line
/// in a free-text detail string. Returns `None` if no such pattern is found.
fn extract_version_info(detail: &str) -> Option<String> {
    let idx = detail.find("Version info:")?;
    let tail = &detail[idx + "Version info:".len()..];
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// Strip the parenthesised suffix from a version string for comparison:
/// `"3910(Minor, Thu …)"` → `"3910"`. Already-stripped values pass through.
fn leading_version_number(v: &str) -> &str {
    match v.find('(') {
        Some(i) => v[..i].trim(),
        None => v.trim(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/signature_package")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()))
    }

    // ── parse_package_information ────────────────────────────────────────────

    #[test]
    fn fresh_device_returns_single_node_with_none_version() {
        let xml = fixture("idp_package_information_fresh.xml");
        let nodes = parse_package_information(&xml).expect("parse");
        assert_eq!(nodes.len(), 1, "standalone => single node");
        assert_eq!(nodes[0].re_name, "", "standalone re_name is empty");
        assert_eq!(
            nodes[0].current_package_version, None,
            "N/A(N/A) normalises to None"
        );
    }

    #[test]
    fn post_install_returns_full_version_text() {
        let xml = fixture("idp_package_information_post_install.xml");
        let nodes = parse_package_information(&xml).expect("parse");
        assert_eq!(nodes.len(), 1);
        let v = nodes[0]
            .current_package_version
            .as_deref()
            .expect("present");
        assert!(v.starts_with("3910"), "version starts with 3910: {v:?}");
        assert!(v.contains("Minor"), "carries Minor tag: {v:?}");
    }

    #[test]
    fn clustered_returns_two_nodes() {
        let xml = fixture("idp_package_information_clustered.xml");
        let nodes = parse_package_information(&xml).expect("parse");
        assert_eq!(nodes.len(), 2, "cluster => two nodes");
        let names: Vec<&str> = nodes.iter().map(|n| n.re_name.as_str()).collect();
        assert!(names.contains(&"node0"), "names={names:?}");
        assert!(names.contains(&"node1"), "names={names:?}");
        // Both nodes are fresh in this fixture.
        assert!(nodes.iter().all(|n| n.current_package_version.is_none()));
    }

    // ── parse_check_server_reply ─────────────────────────────────────────────

    #[test]
    fn check_server_update_available_extracts_version() {
        let xml = fixture("idp_check_server_update_available.xml");
        let v = parse_check_server_reply(&xml, "vsrx-ci-tester").expect("parse");
        assert_eq!(v, "3910");
    }

    #[test]
    fn check_server_at_latest_extracts_same_wire_shape() {
        // Per design Appendix A.3: at_latest and update_available share
        // the same wire shape; only the caller can distinguish them by
        // comparing against current_package_version.
        let xml = fixture("idp_check_server_at_latest.xml");
        let v = parse_check_server_reply(&xml, "vsrx-ci-tester").expect("parse");
        assert_eq!(v, "3910");
    }

    #[test]
    fn check_server_unreachable_returns_server_unreachable_variant() {
        let xml = fixture("idp_check_server_unreachable.xml");
        let err =
            parse_check_server_reply(&xml, "vsrx-ci-tester").expect_err("unreachable must error");
        match err {
            SrxError::SignaturePackageServerUnreachable { router, detail } => {
                assert_eq!(router, "vsrx-ci-tester");
                assert!(
                    detail.contains("Server not reachable"),
                    "detail should carry Junos's message: got {detail:?}"
                );
            }
            other => panic!("expected SignaturePackageServerUnreachable, got {other:?}"),
        }
    }

    #[test]
    fn check_server_missing_version_info_returns_parse_error() {
        let xml = r#"<secpack-download-status format="xml">
            <secpack-download-status-detail>some text without the magic line</secpack-download-status-detail>
        </secpack-download-status>"#;
        let err = parse_check_server_reply(xml, "vsrx-foo").expect_err("missing Version info");
        match err {
            SrxError::Parse(msg) => assert!(
                msg.contains("Version info"),
                "parse error should mention the missing token: {msg:?}"
            ),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn check_server_missing_detail_element_is_schema_mismatch() {
        let xml = r#"<secpack-download-status format="xml"></secpack-download-status>"#;
        let err = parse_check_server_reply(xml, "vsrx-foo").expect_err("missing detail");
        match err {
            SrxError::SchemaMismatch { rpc, element } => {
                assert_eq!(rpc, RPC_CHECK_SERVER);
                assert_eq!(element, "secpack-download-status-detail");
            }
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }

    // ── normalize_version_text ───────────────────────────────────────────────

    #[test]
    fn normalize_version_handles_n_a_variants() {
        assert_eq!(normalize_version_text("N/A(N/A)"), None);
        assert_eq!(normalize_version_text("N/A"), None);
        assert_eq!(normalize_version_text("n/a"), None);
        assert_eq!(normalize_version_text(""), None);
        assert_eq!(normalize_version_text("   "), None);
        assert_eq!(
            normalize_version_text("3910(Minor, Thu …)"),
            Some("3910(Minor, Thu …)".to_string())
        );
    }

    // ── leading_version_number ───────────────────────────────────────────────

    #[test]
    fn leading_version_strips_parens() {
        assert_eq!(leading_version_number("3910(Minor, Thu …)"), "3910");
        assert_eq!(leading_version_number("3910"), "3910");
        assert_eq!(leading_version_number("3712(4.1)"), "3712");
    }

    // ── extract_version_info ─────────────────────────────────────────────────

    #[test]
    fn extract_version_info_pulls_digits_after_colon() {
        let detail = "Successfully retrieved from(https://…).\nVersion info:3910(Minor, …)";
        assert_eq!(extract_version_info(detail).as_deref(), Some("3910"));
    }

    #[test]
    fn extract_version_info_returns_none_when_absent() {
        assert_eq!(extract_version_info("not a check-server reply"), None);
    }
}
