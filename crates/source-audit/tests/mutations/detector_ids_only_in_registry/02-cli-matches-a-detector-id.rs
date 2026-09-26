//! target: crates/cli/src/main.rs
//! by: audit:ids_only_in_their_module
//! ported: 2026-09-22 -- the constant's current path (`locations::homebrew`)
//! why: the same coupling in the CLI, where a hand-written file list would not look
fn sweep_cli_hint(id: &str) -> &'static str {
    if id == swamp_core::locations::homebrew::HOMEBREW_DETECTOR_ID {
        "brew cleanup"
    } else {
        ""
    }
}
