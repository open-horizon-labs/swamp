//! symlinks-never-followed: the gate names the two stat calls
//! `symlink_metadata` and `metadata_following`; there is no plain
//! `metadata` whose following is easy to miss.
fn main() {
    let _ = swamp_core::fs_gate::metadata("/tmp/link");
}
