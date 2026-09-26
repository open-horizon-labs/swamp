//! target: crates/core/src/agents/gemini_cli.rs
//! by: audit:adapters_do_not_reach_gates
//! why: an adapter reaching into locations/ for home resolution instead of using the home it is handed
pub fn sweep_resolve_home(env: &crate::locations::Environment) -> Option<std::path::PathBuf> {
    let registry = crate::locations::Registry::with_builtins();
    let _ = registry;
    let _ = env;
    None
}
