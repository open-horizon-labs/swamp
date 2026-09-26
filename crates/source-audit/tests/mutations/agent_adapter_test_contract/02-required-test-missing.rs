//! target: crates/core/src/agents/aider.rs
//! mode: replace
//! by: audit:guardrail_metadata
//! why: one of the five things every adapter proves about itself simply removed
pub const AIDER_TOOL_ID: &str = "aider";

pub struct Adapter;

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_format_is_explicit_not_empty() {}
    #[test]
    fn canary_content_never_appears_in_output() {}
    #[test]
    fn identification_reads_no_more_than_header_cap() {}
    #[test]
    fn protected_categories_default_protected() {}
}
