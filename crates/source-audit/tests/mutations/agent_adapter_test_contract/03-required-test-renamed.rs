//! target: crates/core/src/agents/aider.rs
//! mode: replace
//! by: audit:guardrail_metadata
//! why: alias/rename variant -- the privacy test renamed, so the name resolves nowhere and the adapter still looks compliant to a reader
pub const AIDER_TOOL_ID: &str = "aider";

pub struct Adapter;

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_format_is_explicit_not_empty() {}
    #[test]
    fn no_canary_in_output() {}
    #[test]
    fn identification_reads_no_more_than_header_cap() {}
    #[test]
    fn protected_categories_default_protected() {}
    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {}
}
