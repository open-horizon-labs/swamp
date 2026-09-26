//! scheduled-refresh-launchagent: the only plist swamp writes is the
//! `TextFile::LaunchAgent` variant (validated to `<label>.plist`); there is
//! no text write to an arbitrary path.
use std::path::Path;
use swamp_core::fs_gate::store;

fn main() {
    let _ = store::write_text(Path::new("/tmp/LaunchAgents/other.plist"), "<plist/>");
}
