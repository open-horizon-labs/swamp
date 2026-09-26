//! target: crates/core/src/fs_events.rs
//! mode: append
//! by: audit:no_unreferenced_public_items
//! why: a second place deciding which refusal a platform without replay gives -- this one always says "unsupported_platform", which tells a Linux user a backend is missing rather than that the kernel keeps no history

/// A convenience wrapper that "helpfully" answers for the caller.
pub fn refusal_for_this_build() -> RefreshRefusal {
    RefreshRefusal::UnsupportedPlatform
}
