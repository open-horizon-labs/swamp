//! target: crates/core/src/scope.rs
//! mode: append
//! by: audit:no_root_inference_sources
//! why: the HISTFILE variable names the shell history file without spelling it
/// Sweep: shell history located through $HISTFILE.
pub fn sweep_histfile() -> Option<std::ffi::OsString> {
    std::env::var_os("HISTFILE")
}
