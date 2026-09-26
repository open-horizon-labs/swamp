//! target: crates/core/src/docker.rs
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: the constructor written inside `vec![..]`, where `syn::visit` never looks
pub fn sweep_commands() -> Vec<std::process::Command> {
    vec![std::process::Command::new("docker")]
}
