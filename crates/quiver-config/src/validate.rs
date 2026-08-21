use crate::error::ValidationIssue;

/// Semantic validation beyond what serde deserialization can express:
/// ranges, cross-field references, format constraints.
///
/// Implementations must return an empty Vec when the value is valid.
/// Never panic on invalid data — report it.
pub trait Validate {
    fn validate(&self) -> Vec<ValidationIssue>;
}

/// Append an issue to `out` when `cond` is false.
///
/// Intended to keep validators flat and obvious:
///
/// ```ignore
/// require(self.font_size_px >= 8, "font_size_px", "must be at least 8", &mut out);
/// ```
pub fn require(cond: bool, path: &str, message: impl Into<String>, out: &mut Vec<ValidationIssue>) {
    if !cond {
        out.push(ValidationIssue {
            path: path.to_string(),
            message: message.into(),
        });
    }
}
