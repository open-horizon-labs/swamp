//! target: crates/core/src/agents/vscode_family.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M37
//! by: audit:gate_paths_only_inside_gates, compile:unsafe_code
//! why: an adapter taking the home directory from the password database (`libc::getpwuid`)
//! blind-spot (old model): environment access is `std::env::*`, `env::*`, `dirs::*`, `home::*` or `home_dir`; `libc` is a dependency of core and `getpwuid(getuid())->pw_dir` is the same un-injectable home
/// Sweep 4: six tools' homes, from the passwd database.
pub fn sweep4_home() -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
        Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
            dir.to_bytes(),
        )))
    }
}
