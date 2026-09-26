//! fsevents-before-full-walk, tui-refresh-preserves-scope: the canned
//! FSEvents replay source exists only under the `testing` feature, so no
//! production path (the TUI live refresh included) can replay a canned
//! answer instead of asking FSEvents.
fn main() {
    let _ = swamp_core::fs_events::testing::CannedSource::default();
}
