//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: other shells and REPLs keep history too (.python_history, .node_repl_history, .histfile)
/// Sweep: other history files.
pub fn sweep_other_histories() -> [&'static str; 3] {
    [".python_history", ".node_repl_history", ".histfile"]
}
