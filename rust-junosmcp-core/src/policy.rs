//! Pure rule-evaluation logic for the blocklist guardrails.
//!
//! `Policy` is built once at startup from the parsed [`Inventory`](crate::Inventory)
//! and is cheap to clone via `Arc`. Tool handlers consult it before any device
//! interaction.

use crate::error::JmcpError;
use crate::inventory::{Action, RuleSpec};
use globset::{Glob, GlobMatcher};

/// Origin of a rule, used for tiebreaking equal-specificity matches and for
/// the human-readable error message on denial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleSource {
    Defaults,
    Device,
}

impl RuleSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Defaults => "defaults",
            Self::Device => "device",
        }
    }
}

/// A glob rule with its compiled matcher and pre-computed specificity score.
#[derive(Debug)]
pub struct CompiledRule {
    pub pattern: String,
    pub action: Action,
    pub source: RuleSource,
    pub matcher: GlobMatcher,
    /// Higher = more specific. Tuple is `(literal_chars, total_len)`.
    pub specificity: (usize, usize),
}

/// Count non-wildcard, non-character-class literal characters in a glob pattern.
/// `*`, `?`, and `[...]` ranges are wildcards; everything else (including
/// escaped characters) counts.
pub(crate) fn count_literal_chars(pattern: &str) -> usize {
    let mut count = 0usize;
    let mut in_class = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if in_class {
            if c == ']' {
                in_class = false;
            }
            continue;
        }
        match c {
            '*' | '?' => continue,
            '[' => {
                in_class = true;
                continue;
            }
            '\\' => {
                if chars.next().is_some() {
                    count += 1;
                }
            }
            _ => count += 1,
        }
    }
    count
}

/// Compile a list of `RuleSpec`s into `CompiledRule`s, attaching the given
/// `source` and a scope label used in compile-time error messages.
pub(crate) fn compile_rules(
    rules: &[RuleSpec],
    scope: &str,
    source: RuleSource,
) -> Result<Vec<CompiledRule>, JmcpError> {
    rules
        .iter()
        .map(|r| {
            let glob = Glob::new(&r.pattern).map_err(|e| JmcpError::BlocklistRuleInvalid {
                scope: scope.to_string(),
                pattern: r.pattern.clone(),
                source: e,
            })?;
            let literal_chars = count_literal_chars(&r.pattern);
            Ok(CompiledRule {
                pattern: r.pattern.clone(),
                action: r.action,
                source,
                matcher: glob.compile_matcher(),
                specificity: (literal_chars, r.pattern.len()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(action: Action, pattern: &str) -> RuleSpec {
        RuleSpec {
            action,
            pattern: pattern.into(),
        }
    }

    #[test]
    fn count_literals_handles_wildcards_and_classes() {
        assert_eq!(count_literal_chars("request system reboot"), 21);
        assert_eq!(count_literal_chars("request system *"), 15);
        assert_eq!(count_literal_chars("*"), 0);
        assert_eq!(count_literal_chars("?abc"), 3);
        assert_eq!(count_literal_chars("ab[cd]ef"), 4); // class doesn't count
        assert_eq!(count_literal_chars(r"\*literal"), 8); // escaped * counts as literal
    }

    #[test]
    fn compile_rules_succeeds_on_valid_globs() {
        let r = vec![
            spec(Action::Deny, "request system *"),
            spec(Action::Allow, "show version"),
        ];
        let compiled = compile_rules(&r, "test", RuleSource::Defaults).unwrap();
        assert_eq!(compiled.len(), 2);
        assert_eq!(compiled[0].specificity, (15, 16));
        assert_eq!(compiled[0].source, RuleSource::Defaults);
    }

    #[test]
    fn compile_rules_errors_with_scope_on_bad_glob() {
        let r = vec![spec(Action::Deny, "[unterminated")];
        let err = compile_rules(&r, "_blocklist_defaults.commands", RuleSource::Defaults)
            .unwrap_err();
        match err {
            JmcpError::BlocklistRuleInvalid {
                scope, pattern, ..
            } => {
                assert_eq!(scope, "_blocklist_defaults.commands");
                assert_eq!(pattern, "[unterminated");
            }
            _ => panic!("expected BlocklistRuleInvalid, got {err:?}"),
        }
    }
}
