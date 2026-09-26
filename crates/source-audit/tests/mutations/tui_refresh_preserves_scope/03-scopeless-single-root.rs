//! target: crates/tui/src/units.rs
//! by: audit:gate_paths_only_inside_gates
//! why: a per-root refresh that drops the scope, so a pruned external location comes back
pub fn sweep_one_root(root: &std::path::Path) -> anyhow::Result<swamp_core::report::Report> {
    let r = swamp_core::report::report_single_root(root, 1_000)?;
    Ok(r)
}
