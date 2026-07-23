use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    tool::{CapabilityMode, ToolCancellation, ToolEffect},
    types::ToolCall,
};

/// Maximum number of remembered permission rules attached to one Agent session.
pub const MAX_TOOL_PERMISSION_RULES: usize = 256;
/// Maximum UTF-8 size of one tool name in a permission rule.
pub const MAX_TOOL_PERMISSION_NAME_BYTES: usize = 256;
/// Maximum UTF-8 size of one argument pattern in a permission rule.
pub const MAX_TOOL_PERMISSION_PATTERN_BYTES: usize = 4096;
/// Maximum target size eligible for a remembered wildcard rule. Larger calls
/// can still be approved once, but do not spend unbounded work in matching.
pub const MAX_TOOL_PERMISSION_TARGET_BYTES: usize = 64 * 1024;

/// A session-scoped exception to the Agent's automatic capability boundary.
///
/// Tool names are exact. `pattern = None` allows every invocation of that
/// tool; a pattern is interpreted by the tool implementation. Built-in Bash
/// supports exact, legacy `:*` prefix, and `*` wildcard patterns.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPermissionRule {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

impl ToolPermissionRule {
    pub fn new(tool_name: impl Into<String>, pattern: Option<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            pattern,
        }
    }

    pub fn for_tool(tool_name: impl Into<String>) -> Self {
        Self::new(tool_name, None)
    }

    pub fn validate(&self) -> Result<(), ToolPermissionRuleError> {
        if self.tool_name.trim().is_empty() {
            return Err(ToolPermissionRuleError::EmptyToolName);
        }
        if self.tool_name.len() > MAX_TOOL_PERMISSION_NAME_BYTES {
            return Err(ToolPermissionRuleError::ToolNameTooLong {
                actual: self.tool_name.len(),
                maximum: MAX_TOOL_PERMISSION_NAME_BYTES,
            });
        }
        if let Some(pattern) = &self.pattern {
            if pattern.trim().is_empty() {
                return Err(ToolPermissionRuleError::EmptyPattern);
            }
            if pattern.len() > MAX_TOOL_PERMISSION_PATTERN_BYTES {
                return Err(ToolPermissionRuleError::PatternTooLong {
                    actual: pattern.len(),
                    maximum: MAX_TOOL_PERMISSION_PATTERN_BYTES,
                });
            }
        }
        Ok(())
    }
}

impl fmt::Display for ToolPermissionRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.pattern {
            Some(pattern) => write!(formatter, "{}({pattern})", self.tool_name),
            None => formatter.write_str(&self.tool_name),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ToolPermissionRuleError {
    #[error("permission rule tool name must not be empty")]
    EmptyToolName,
    #[error("permission rule tool name is {actual} bytes; maximum is {maximum}")]
    ToolNameTooLong { actual: usize, maximum: usize },
    #[error("permission rule pattern must not be empty")]
    EmptyPattern,
    #[error("permission rule pattern is {actual} bytes; maximum is {maximum}")]
    PatternTooLong { actual: usize, maximum: usize },
}

/// One tool invocation that crossed the Agent's automatic capability boundary.
#[derive(Clone, Debug)]
pub struct ToolPermissionRequest {
    pub call: ToolCall,
    pub effect: ToolEffect,
    pub capability_mode: CapabilityMode,
    pub suggestions: Vec<ToolPermissionRule>,
    pub cancellation: ToolCancellation,
}

/// Host decision for a pending tool permission request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolPermissionDecision {
    /// Permit only the current tool call.
    AllowOnce,
    /// Permit the current call and remember a matching rule in this session.
    AllowForSession { rule: ToolPermissionRule },
    /// Return an error tool result without invoking the tool.
    Deny { message: String },
}

/// Host boundary used when a registered tool needs authority above the current
/// [`CapabilityMode`].
///
/// The library does not install an approver. Without one, capability violations
/// retain the original fail-closed behavior.
#[async_trait]
pub trait ToolPermissionApprover: Send + Sync {
    async fn decide(&self, request: ToolPermissionRequest) -> ToolPermissionDecision;
}

/// Matches an anchored permission pattern. `*` consumes any number of Unicode
/// scalar values, `\*` represents a literal asterisk, and `\\` a literal
/// backslash. A sole trailing ` *` is optional, so `ls *` matches both `ls`
/// and `ls -la`.
pub fn matches_permission_pattern(pattern: &str, value: &str) -> bool {
    if pattern.len() > MAX_TOOL_PERMISSION_PATTERN_BYTES
        || value.len() > MAX_TOOL_PERMISSION_TARGET_BYTES
    {
        return false;
    }
    let mut tokens = tokenize_pattern(pattern);
    let trailing_optional = tokens.len() >= 2
        && tokens.last() == Some(&PatternToken::Wildcard)
        && tokens[tokens.len() - 2] == PatternToken::Literal(' ')
        && tokens
            .iter()
            .filter(|token| **token == PatternToken::Wildcard)
            .count()
            == 1;
    if wildcard_match(&tokens, value) {
        return true;
    }
    if trailing_optional {
        tokens.truncate(tokens.len() - 2);
        return wildcard_match(&tokens, value);
    }
    false
}

/// Escapes a concrete permission target so it is matched literally when used
/// as a [`ToolPermissionRule::pattern`].
pub fn escape_permission_pattern(value: &str) -> String {
    value.replace('\\', "\\\\").replace('*', "\\*")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PatternToken {
    Literal(char),
    Wildcard,
}

fn tokenize_pattern(pattern: &str) -> Vec<PatternToken> {
    let mut tokens = Vec::with_capacity(pattern.chars().count());
    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some('*') => tokens.push(PatternToken::Literal('*')),
                Some('\\') => tokens.push(PatternToken::Literal('\\')),
                Some(next) => {
                    tokens.push(PatternToken::Literal('\\'));
                    tokens.push(PatternToken::Literal(next));
                }
                None => tokens.push(PatternToken::Literal('\\')),
            }
        } else if character == '*' {
            if tokens.last() != Some(&PatternToken::Wildcard) {
                tokens.push(PatternToken::Wildcard);
            }
        } else {
            tokens.push(PatternToken::Literal(character));
        }
    }
    tokens
}

fn wildcard_match(pattern: &[PatternToken], value: &str) -> bool {
    let value = value.chars().collect::<Vec<_>>();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut wildcard_index = None;
    let mut wildcard_value_index = 0;

    while value_index < value.len() {
        match pattern.get(pattern_index) {
            Some(PatternToken::Literal(expected)) if *expected == value[value_index] => {
                pattern_index += 1;
                value_index += 1;
            }
            Some(PatternToken::Wildcard) => {
                wildcard_index = Some(pattern_index);
                pattern_index += 1;
                wildcard_value_index = value_index;
            }
            _ if wildcard_index.is_some() => {
                wildcard_value_index += 1;
                value_index = wildcard_value_index;
                pattern_index = wildcard_index.expect("checked above") + 1;
            }
            _ => return false,
        }
    }
    pattern[pattern_index..]
        .iter()
        .all(|token| *token == PatternToken::Wildcard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_patterns_are_anchored_and_support_escaping() {
        assert!(matches_permission_pattern("ls *", "ls"));
        assert!(matches_permission_pattern("ls *", "ls -la src"));
        assert!(!matches_permission_pattern("ls *", "lsof"));
        assert!(matches_permission_pattern(r"echo \*", "echo *"));
        assert!(!matches_permission_pattern(r"echo \*", "echo anything"));
        assert!(matches_permission_pattern(
            "git * status",
            "git -C . status"
        ));
        assert!(!matches_permission_pattern(
            "git * status",
            "git status --short"
        ));
        assert!(matches_permission_pattern(" echo", " echo"));
        assert!(!matches_permission_pattern(" echo", "echo"));
        assert!(!matches_permission_pattern(
            "*",
            &"x".repeat(MAX_TOOL_PERMISSION_TARGET_BYTES + 1)
        ));
    }

    #[test]
    fn validates_rule_bounds() {
        assert!(ToolPermissionRule::for_tool("bash").validate().is_ok());
        assert_eq!(
            ToolPermissionRule::new("bash", Some(" ".to_owned())).validate(),
            Err(ToolPermissionRuleError::EmptyPattern)
        );
    }

    #[test]
    fn escapes_literal_wildcards_and_backslashes() {
        let value = r"echo * C:\\tmp";
        let pattern = escape_permission_pattern(value);
        assert!(matches_permission_pattern(&pattern, value));
        assert!(!matches_permission_pattern(&pattern, r"echo file C:\\tmp"));
    }
}
