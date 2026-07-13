//! Audit event vocabulary: outcomes, safe metadata values, field-name constants.

use std::fmt::Display;

/// Terminal outcome of an audited tool call.
#[derive(Debug, Clone)]
pub enum AuditOutcome {
    /// Handler completed successfully.
    Succeeded,
    /// Handler returned an error. `kind` is a stable category; `msg` is a
    /// bounded, non-secret Display of the error.
    Failed { kind: &'static str, msg: String },
    /// Authorization denied the call before work began.
    Denied { reason: &'static str },
    /// Guard dropped without an outcome set (client cancel / disconnect).
    Unsettled,
}

/// A safe, non-secret metadata value.
#[derive(Debug, Clone)]
pub enum AuditValue {
    Str(String),
    U64(u64),
    Bool(bool),
}

impl From<&str> for AuditValue { fn from(v: &str) -> Self { AuditValue::Str(v.to_string()) } }
impl From<String> for AuditValue { fn from(v: String) -> Self { AuditValue::Str(v) } }
impl From<u64> for AuditValue { fn from(v: u64) -> Self { AuditValue::U64(v) } }
impl From<usize> for AuditValue { fn from(v: usize) -> Self { AuditValue::U64(v as u64) } }
impl From<bool> for AuditValue { fn from(v: bool) -> Self { AuditValue::Bool(v) } }

impl Display for AuditValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditValue::Str(s) => write!(f, "{s}"),
            AuditValue::U64(n) => write!(f, "{n}"),
            AuditValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// Truncate an error Display to a bounded length for the audit event.
pub fn bounded_error(e: impl Display) -> String {
    let s = e.to_string();
    if s.len() <= 512 { s } else { format!("{}…", &s[..512]) }
}
