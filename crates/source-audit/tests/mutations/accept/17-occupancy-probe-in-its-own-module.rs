//! target: crates/core/src/occupancy.rs
//! expect: accept
//! source: accept
//! why: the occupancy module running `lsof` through the counted gate spawn
/// Accept: a counted, named, read-only probe.
fn sweep_accept_lsof(p: &std::path::Path) -> bool {
    crate::fs_gate::spawn::run(
        crate::fs_gate::spawn::Program::Lsof,
        [std::ffi::OsStr::new("--"), p.as_os_str()],
        std::time::Duration::from_secs(5),
    )
    .map(|o| o.success())
    .unwrap_or(false)
}
