//! #190: gix reads the global git config on every open. A FIFO there
//! must not park the pass: repositories are then opened isolated.
//! Its own test binary, since the check is made once per process.

#![cfg(unix)]

use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn a_fifo_global_gitconfig_does_not_park_repo_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("proj");
    std::fs::create_dir_all(&r).unwrap();
    swamp_core::work_counters::record_spawn();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&r)
            .status()
            .unwrap()
            .success()
    );
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let gc = home.join(".gitconfig");
    let c = std::ffi::CString::new(gc.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    // SAFETY: single-test binary; set before any thread reads HOME.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = swamp_core::signals::compute_signals_raw(&r, 0);
        let _ = swamp_core::ignore::untracked_content(&r, 10, 1000);
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("parked on a FIFO ~/.gitconfig (#190)");
}
