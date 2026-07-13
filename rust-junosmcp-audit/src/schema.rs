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

impl From<&str> for AuditValue {
    fn from(v: &str) -> Self {
        AuditValue::Str(v.to_string())
    }
}
impl From<String> for AuditValue {
    fn from(v: String) -> Self {
        AuditValue::Str(v)
    }
}
impl From<u64> for AuditValue {
    fn from(v: u64) -> Self {
        AuditValue::U64(v)
    }
}
impl From<usize> for AuditValue {
    fn from(v: usize) -> Self {
        AuditValue::U64(v as u64)
    }
}
impl From<bool> for AuditValue {
    fn from(v: bool) -> Self {
        AuditValue::Bool(v)
    }
}

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
/// Truncates at the largest char boundary <= 512 bytes to avoid slicing
/// mid-codepoint in multi-byte UTF-8 strings.
pub fn bounded_error(e: impl Display) -> String {
    let s = e.to_string();
    if s.len() <= 512 {
        s
    } else {
        // Truncate at the largest char boundary <= 512 bytes, then append an ellipsis.
        let cut = (0..=512)
            .rev()
            .find(|&i| s.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}…", &s[..cut])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_error_truncates_long_ascii() {
        // >512-byte ASCII error should be truncated and end with …
        let long_ascii = "A".repeat(600);
        let result = bounded_error(&long_ascii);
        assert!(result.ends_with('…'));
        assert!(result.len() <= 515); // 512 bytes + "…" (3 bytes in UTF-8)
        assert_eq!(result.len(), 512 + "…".len());
    }

    #[test]
    fn bounded_error_truncates_multibyte_utf8() {
        // Multibyte UTF-8 error should NOT panic and yield valid string with …
        // "é" is 2 bytes in UTF-8; 400 chars = 800 bytes
        let multibyte = "é".repeat(400);
        assert_eq!(multibyte.len(), 800);
        let result = bounded_error(&multibyte);
        assert!(result.ends_with('…'));
        assert!(result.len() <= 515);
        // Must be valid UTF-8 (implicitly validated by String type, but let's be explicit)
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn bounded_error_short_strings_unchanged() {
        let short = "error";
        assert_eq!(bounded_error(short), "error");

        let exactly_512 = "X".repeat(512);
        assert_eq!(bounded_error(&exactly_512), exactly_512);
    }
}
