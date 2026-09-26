//! target: crates/core/src/agents/vscode_family.rs
//! mode: append
//! by: audit:adapters_do_not_reach_gates
//! why: re-review 3 sweep -- the home taken from `$HOME` inside the shared VS Code family module (blind spot: `AGENT_NON_ADAPTERS` exempts vscode_family.rs and pi_family.rs, which are the per-tool mechanics for Cursor, Windsurf, Continue, Cline, Pi and Oh My Pi)
/// Sweep: six tools' homes now come from the environment.
pub fn sweep_home() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
}
