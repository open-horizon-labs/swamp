//! target: crates/core/src/agents/aider.rs
//! mode: replace
//! by: audit:guardrail_metadata
//! why: sweep slip -- a required test that is `#[ignore]`d still resolves by name, so the contract reads as met and nothing runs
pub const AIDER_TOOL_ID: &str = "aider";

pub struct Adapter;

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_format_is_explicit_not_empty() {}
    #[test]
    fn canary_content_never_appears_in_output() {}
    #[test]
    #[ignore]
    fn identification_reads_no_more_than_header_cap() {}
    #[test]
    fn protected_categories_default_protected() {}
    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {}
}
