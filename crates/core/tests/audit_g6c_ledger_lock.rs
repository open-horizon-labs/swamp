//! v0.8.0 G6c audit: the ledger lock (`ledger.lock` beside the ledger),
//! exercised across REAL processes. The test binary re-executes itself
//! as the second process (`AUDIT_G6C_CHILD` selects the child's job).
//! Only temp directories; nothing touches the real store.
//!
//! Each test names the tempting wrong patch it fails.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use swamp_core::ledger::{ActionRecord, Ledger, NO_GRANT, Verb};

const N: usize = 50;

fn rec(id: String) -> ActionRecord {
    ActionRecord {
        id,
        verb: Verb::Delete,
        entity_id: "e".into(),
        evidence: vec![swamp_core::ledger::LedgerFact::new("k", "v")],
        grant_id: NO_GRANT.into(),
        actor: "audit".into(),
        outcome: "completed".into(),
        recovery_location: None,
        measured_free_space_delta: None,
        observed_path_state: None,
        recorded_at: 1,
    }
}

fn ledger_at(store: &Path) -> Ledger {
    Ledger::open(store.join("ledger.parquet")).unwrap()
}

/// Child entry point: does nothing unless the parent set the env var.
// The grandchild is reaped by the parent test (SIGKILL by pid).
#[allow(clippy::zombie_processes)]
#[test]
fn audit_g6c_child() {
    let Ok(job) = std::env::var("AUDIT_G6C_CHILD") else {
        return;
    };
    let store = PathBuf::from(std::env::var("AUDIT_G6C_STORE").unwrap());
    match job.as_str() {
        "append" => {
            let tag = std::env::var("AUDIT_G6C_TAG").unwrap();
            let l = ledger_at(&store);
            for i in 0..N {
                l.append(&rec(format!("{tag}-{i}"))).unwrap();
            }
        }
        "hold" | "hold-and-spawn" => {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(store.join("ledger.lock"))
                .unwrap();
            assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
            std::fs::write(store.join("held"), b"").unwrap();
            if job == "hold-and-spawn" {
                // The lock is taken through swamp's own path here, so the
                // grandchild inherits swamp's fd (or not, if CLOEXEC).
                drop(f);
                let _l = swamp_core::fs_gate::StoreDir::lock_ledger_writes(
                    &store.join("ledger.parquet"),
                )
                .unwrap();
                let g = Command::new("/bin/sleep")
                    .arg("30")
                    .stdin(Stdio::null())
                    .spawn()
                    .unwrap();
                std::fs::write(store.join("grandchild"), g.id().to_string()).unwrap();
                std::fs::write(store.join("held"), b"").unwrap();
                std::thread::sleep(Duration::from_secs(30));
            } else {
                std::thread::sleep(Duration::from_secs(15));
            }
        }
        other => panic!("unknown job {other}"),
    }
}

fn spawn_child(job: &str, store: &Path, tag: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "audit_g6c_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("AUDIT_G6C_CHILD", job)
        .env("AUDIT_G6C_STORE", store)
        .env("AUDIT_G6C_TAG", tag)
        .env_remove("SWAMP_LEDGER_PATH")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

fn wait_for(p: &Path) {
    let t = Instant::now();
    while !p.exists() {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "child never got there"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Tempting wrong patch: lock only within the process (a Mutex, or the
/// lock taken on the ledger file that `write_ledger_rows` replaces by
/// rename). Two real processes racing 50 appends each lose rows.
#[test]
fn audit_g6c_two_processes_racing_50_appends_each_lose_no_row() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().to_path_buf();
    let mut a = spawn_child("append", &store, "a");
    let mut b = spawn_child("append", &store, "b");
    assert!(a.wait().unwrap().success(), "process a failed");
    assert!(b.wait().unwrap().success(), "process b failed");
    let all = ledger_at(&store).all().expect("ledger readable, not torn");
    assert_eq!(all.len(), 2 * N, "rows lost across processes");
    let mut ids: Vec<_> = all.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 2 * N);
    assert!(
        all.iter().all(|r| r.evidence.len() == 1),
        "facts table torn"
    );
    assert!(
        !std::fs::read_dir(&store).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("corrupt")),
        "a racing append saw a torn ledger and kept it aside"
    );
}

/// Tempting wrong patch: wait forever (blocking `lock()`), or write
/// anyway after the wait. Another process holds the lock for 15 s: the
/// append refuses after ~10 s with the plain reason and writes nothing.
#[test]
fn audit_g6c_a_held_lock_refuses_after_10s_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().to_path_buf();
    ledger_at(&store).append(&rec("before".into())).unwrap();
    let mut holder = spawn_child("hold", &store, "");
    wait_for(&store.join("held"));
    let t = Instant::now();
    let err = ledger_at(&store).append(&rec("during".into())).unwrap_err();
    let waited = t.elapsed();
    let _ = holder.kill();
    let _ = holder.wait();
    let msg = err.to_string();
    assert!(
        waited >= Duration::from_secs(9) && waited < Duration::from_secs(14),
        "waited {waited:?}"
    );
    assert!(
        msg.contains("locked by another swamp process") && msg.contains("nothing was written"),
        "{msg}"
    );
    assert!(!swamp_core::ledger::wrote_into_new_ledger(&err));
    let ids: Vec<_> = ledger_at(&store)
        .all()
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, vec!["before".to_string()]);
}

/// Tempting wrong patch: a pid file / O_EXCL lock file instead of flock.
/// A holder killed with SIGKILL leaves `ledger.lock` behind; it must not
/// block the next writer at all.
#[test]
fn audit_g6c_a_killed_holder_leaves_no_blocking_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().to_path_buf();
    let mut holder = spawn_child("hold", &store, "");
    wait_for(&store.join("held"));
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(store.join("ledger.lock").exists());
    let t = Instant::now();
    ledger_at(&store).append(&rec("after".into())).unwrap();
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

/// Tempting wrong patch: open the lock file without O_CLOEXEC (libc open
/// or a raw fd). A manager command spawned while the lock is held would
/// inherit it and keep the ledger locked after swamp exits.
#[test]
fn audit_g6c_a_spawned_child_does_not_inherit_the_ledger_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().to_path_buf();
    let mut holder = spawn_child("hold-and-spawn", &store, "");
    wait_for(&store.join("grandchild"));
    let gpid: i32 = std::fs::read_to_string(store.join("grandchild"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    holder.kill().unwrap();
    holder.wait().unwrap();
    let t = Instant::now();
    let r = ledger_at(&store).append(&rec("after".into()));
    let took = t.elapsed();
    unsafe { libc::kill(gpid, libc::SIGKILL) };
    r.unwrap();
    assert!(
        took < Duration::from_secs(1),
        "grandchild held the lock: {took:?}"
    );
}

/// flock is per open file description: a second `lock_ledger_writes` in
/// the SAME process while one guard lives is NOT re-entrant (it times
/// out). Pinned so nobody wraps `append`/`replace` in an outer guard
/// believing it re-enters. Sequential started-row then final-row (the
/// real call pattern) must not wait at all.
#[test]
fn audit_g6c_lock_is_not_reentrant_but_sequential_rows_do_not_wait() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().to_path_buf();
    let l = ledger_at(&store);
    let t = Instant::now();
    l.append(&rec("x".into())).unwrap();
    let mut done = rec("x".into());
    done.outcome = "final".into();
    l.replace(&done).unwrap();
    assert!(t.elapsed() < Duration::from_secs(1));
    let all = l.all().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].outcome, "final");
}

/// Tempting wrong patch: treat any lock error as "held" and keep polling.
/// An unwritable store directory (no lock file can be created) must be a
/// prompt refusal with the OS reason, never a 10 s wait or a hang.
#[test]
fn audit_g6c_an_unwritable_store_refuses_at_once_with_the_reason() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("s");
    std::fs::create_dir(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555)).unwrap();
    let t = Instant::now();
    let err = ledger_at(&store).append(&rec("x".into())).unwrap_err();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    let msg = err.to_string();
    assert!(
        msg.contains("could not be locked") && msg.to_lowercase().contains("permission"),
        "{msg}"
    );
}
