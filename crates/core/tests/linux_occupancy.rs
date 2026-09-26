//! #86 on a real Linux kernel: the procfs occupancy probe against real
//! processes. The fail-closed rules themselves (another PID namespace's
//! procfs, an unreadable process of this user, the time bound) are unit
//! tests in `occupancy.rs` against fixture trees, because a CI runner
//! cannot be made to have them on demand; this file is the evidence
//! that the probe sees what real processes hold.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use swamp_core::occupancy::{OccupancyState, probe_path};

/// One test at a time: one of them clears `PATH`, which every other
/// test here needs to spawn its child.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let target = std::fs::canonicalize(tmp.path()).unwrap().join("target");
    std::fs::create_dir_all(target.join("debug/deps")).unwrap();
    std::fs::write(target.join("debug/deps/libx.rlib"), vec![1u8; 4096]).unwrap();
    (tmp, target)
}

/// Waits until the probe gives the wanted answer, or returns the last
/// one: a child's `/proc` entries appear shortly after spawn.
fn settle(path: &Path, want: impl Fn(&OccupancyState) -> bool) -> OccupancyState {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = probe_path(path);
        if want(&got) || Instant::now() > deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_process_whose_cwd_is_inside_the_unit_occupies_it_until_it_exits() {
    let _serial = serial();
    let (_tmp, target) = fixture();
    assert_eq!(probe_path(&target), OccupancyState::Free);
    let child = Command::new("sleep")
        .arg("30")
        .current_dir(target.join("debug"))
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn sleep");
    let mut guard = Killed(child);
    let got = settle(&target, |s| matches!(s, OccupancyState::Occupied(_)));
    assert_eq!(
        got,
        OccupancyState::Occupied(target.join("debug")),
        "{got:?}"
    );
    let _ = guard.0.kill();
    let _ = guard.0.wait();
    let got = settle(&target, |s| *s == OccupancyState::Free);
    assert_eq!(
        got,
        OccupancyState::Free,
        "the process exited, the unit is free"
    );
}

#[test]
fn a_process_holding_a_descendant_file_open_occupies_the_whole_unit() {
    let _serial = serial();
    let (_tmp, target) = fixture();
    let file = target.join("debug/deps/libx.rlib");
    let child = Command::new("sh")
        .arg("-c")
        .arg("exec 3<\"$1\"; exec sleep 30")
        .arg("sh")
        .arg(&file)
        .current_dir("/")
        .spawn()
        .expect("spawn sh");
    let _guard = Killed(child);
    let got = settle(&target, |s| matches!(s, OccupancyState::Occupied(_)));
    assert_eq!(got, OccupancyState::Occupied(file.clone()), "{got:?}");
    // The file itself, probed directly, is occupied too.
    assert_eq!(probe_path(&file), OccupancyState::Occupied(file));
}

/// This process counts: the existing propose/execute test holds the
/// unit open from the test process itself and relies on the refusal.
#[test]
fn this_process_holding_the_unit_counts() {
    let _serial = serial();
    let (_tmp, target) = fixture();
    let _held = std::fs::File::open(target.join("debug/deps/libx.rlib")).unwrap();
    assert!(matches!(probe_path(&target), OccupancyState::Occupied(_)));
}

/// Processes starting and exiting throughout the scan are a race the
/// probe must survive without calling it "unknown" or "occupied": an
/// exited process holds nothing.
#[test]
fn processes_exiting_during_the_scan_do_not_make_the_answer_unknown() {
    let _serial = serial();
    let (_tmp, target) = fixture();
    let churn = Command::new("sh")
        .arg("-c")
        .arg("i=0; while [ $i -lt 400 ]; do /bin/true & i=$((i+1)); done; wait")
        .current_dir("/")
        .spawn()
        .expect("spawn churn");
    let _guard = Killed(churn);
    for _ in 0..20 {
        assert_eq!(probe_path(&target), OccupancyState::Free);
    }
}

/// No `lsof` is involved on Linux: the probe answers with an empty PATH.
#[test]
fn the_linux_probe_needs_no_lsof() {
    let _serial = serial();
    let (_tmp, target) = fixture();
    let saved = std::env::var_os("PATH");
    // SAFETY: this test binary's other tests spawn with absolute paths
    // or before this point; PATH is restored below.
    unsafe { std::env::set_var("PATH", "") };
    let got = probe_path(&target);
    unsafe {
        match saved {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }
    assert_eq!(got, OccupancyState::Free);
}

/// A nested PID namespace that mounts its own `/proc` cannot see the
/// host's processes, and from inside it that `/proc` is internally
/// consistent -- so it cannot be told apart from the host. Recorded
/// here as a limit, with the evidence of whether this runner can even
/// construct one, rather than claimed as covered.
#[test]
fn a_nested_pid_namespace_is_a_recorded_limit() {
    let _serial = serial();
    let out = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount-proc",
            "true",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => eprintln!(
            "LIMIT a_nested_pid_namespace: this runner can create one; a probe inside it sees \
             only that namespace's processes (docs/platform.md, Occupancy)"
        ),
        Ok(o) => eprintln!(
            "SKIP a_nested_pid_namespace: unshare refused ({}); unprivileged user namespaces \
             are restricted here",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!("SKIP a_nested_pid_namespace: unshare unavailable ({e})"),
    }
}
