//! target: crates/tui/src/units.rs
//! by: audit:ids_only_in_their_module
//! ported: 2026-09-22 -- the constant's current path (`locations::rustup`)
//! why: the same coupling in the TUI, which is the surface that most often grows a special case
fn sweep_tui_icon(id: &str) -> &'static str {
    match id {
        swamp_core::locations::rustup::RUSTUP_DETECTOR_ID => "rust",
        _ => "",
    }
}
