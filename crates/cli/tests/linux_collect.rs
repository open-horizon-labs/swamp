//! #82 end to end on a real Linux kernel, through the built binary: a
//! user-started `swamp collect` lets `swamp observe` walk only what
//! changed while it runs, and every way that stops being true -- the
//! collector killed, a change made while it was down, a fresh epoch --
//! is a full walk naming why. The incremental result is held to a full
//! walk of the same tree in a fresh store.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

struct Collector(Child);

impl Drop for Collector {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_collector(root: &Path, store: &Path, home: &Path) -> Collector {
    let child = Command::new(bin())
        .arg("collect")
        .arg(root)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn swamp collect");
    Collector(child)
}

/// The pid the root's checkpoint names (`continuity/<id>.parquet`, R18b
/// -- read through the same reader the CLI's `collect --status` uses).
fn checkpoint_pid(store: &Path, root: &Path) -> Option<u32> {
    let p = swamp_core::continuity::paths(store, root);
    swamp_core::continuity::read_checkpoint(&p).map(|c| c.pid)
}

fn wait_for_checkpoint(store: &Path, root: &Path, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if checkpoint_pid(store, root) == Some(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("collector {pid} wrote no checkpoint");
}

/// `swamp observe <root>`'s machine-readable line.
fn observe(root: &Path, store: &Path, home: &Path, full: bool) -> String {
    let mut cmd = Command::new(bin());
    cmd.arg("observe").arg(root);
    if full {
        cmd.arg("--full");
    }
    let out = cmd
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe");
    assert!(
        out.status.success(),
        "observe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("observed_at="))
        .expect("an observe line")
        .to_string()
}

fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|f| f.strip_prefix(&format!("{key}=")))
        .unwrap_or("")
}

fn tree(root: &Path) {
    for p in ["proj/src", "proj/target/debug/deps", "other/node_modules/x"] {
        std::fs::create_dir_all(root.join(p)).unwrap();
    }
    std::fs::write(root.join("proj/Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::write(root.join("proj/src/lib.rs"), "").unwrap();
    std::fs::write(
        root.join("proj/target/debug/deps/a.rlib"),
        vec![1u8; 20_000],
    )
    .unwrap();
    std::fs::write(root.join("other/node_modules/x/i.js"), vec![2u8; 9_000]).unwrap();
    // Real checkouts: the incremental merge is exercised where it is
    // used. (A root with no checkout at all -- every byte unowned -- does
    // not re-measure changed unowned directories incrementally on either
    // platform; that is a pre-existing gap recorded in the session note,
    // not something a collector changes.)
    for p in ["proj", "other"] {
        let ok = Command::new("git")
            .args(["init", "-q", "."])
            .current_dir(root.join(p))
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git init {p}");
    }
}

#[test]
fn a_collector_makes_observations_incremental_only_while_it_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let (root, store, home) = (base.join("src"), base.join("store"), base.join("home"));
    std::fs::create_dir_all(&home).unwrap();
    tree(&root);

    let c1 = start_collector(&root, &store, &home);
    wait_for_checkpoint(&store, &root, c1.0.id());
    std::thread::sleep(Duration::from_millis(1100));

    // A fresh store's first observation is full: it records the
    // classification rules and anchors the cursor. With the collector
    // already alive and checkpointed *before* this first call, that one
    // full walk is enough to start vouching -- the very next observation
    // (nothing having changed since) is already incremental, with
    // nothing to walk.
    let first = observe(&root, &store, &home, false);
    assert_eq!(field(&first, "mode"), "full", "{first}");
    std::thread::sleep(Duration::from_millis(1100));
    let anchor = observe(&root, &store, &home, false);
    assert_eq!(field(&anchor, "mode"), "incremental", "{anchor}");
    assert_eq!(field(&anchor, "changed_dirs"), "0", "{anchor}");

    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(
        root.join("proj/target/debug/deps/b.rlib"),
        vec![3u8; 40_000],
    )
    .unwrap();
    let second = observe(&root, &store, &home, false);
    assert_eq!(field(&second, "mode"), "incremental", "{second}");
    assert_ne!(field(&second, "changed_dirs"), "0", "{second}");

    // The collector dies (SIGKILL: no clean shutdown, no final flush),
    // and a change happens while nothing watches.
    drop(c1);
    std::fs::write(
        root.join("other/node_modules/x/while-stopped.js"),
        vec![4u8; 30_000],
    )
    .unwrap();
    let third = observe(&root, &store, &home, false);
    assert_eq!(field(&third, "mode"), "full", "{third}");
    assert_eq!(field(&third, "reason"), "collector_stopped", "{third}");

    // A new collector is a new epoch: the stored observation predates
    // it, so the first run under it walks fully and says so.
    let c2 = start_collector(&root, &store, &home);
    wait_for_checkpoint(&store, &root, c2.0.id());
    std::thread::sleep(Duration::from_millis(1100));
    let fourth = observe(&root, &store, &home, false);
    assert_eq!(field(&fourth, "reason"), "live_watch_gap", "{fourth}");

    std::thread::sleep(Duration::from_millis(1100));
    std::fs::create_dir_all(root.join("proj/target/debug/incremental/s1")).unwrap();
    std::fs::write(
        root.join("proj/target/debug/incremental/s1/q.bin"),
        vec![5u8; 12_000],
    )
    .unwrap();
    let fifth = observe(&root, &store, &home, false);
    assert_eq!(field(&fifth, "mode"), "incremental", "{fifth}");

    // Equivalence: the same tree, walked fully into a fresh store.
    let fresh = base.join("fresh-store");
    let reference = observe(&root, &fresh, &home, true);
    assert_eq!(
        field(&fifth, "walked_total"),
        field(&reference, "walked_total"),
        "incremental {fifth} vs full {reference}"
    );

    // A clean stop records itself; status then says the collector is
    // not running.
    let pid = c2.0.id();
    // SAFETY: plain kill(2) of our own child.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    let mut c2 = c2;
    let status = c2.0.wait().unwrap();
    assert!(status.success(), "SIGTERM is a clean stop: {status:?}");
    let out = Command::new(bin())
        .args(["collect", "--status", "--json"])
        .arg(&root)
        .env("SWAMP_DIR", &store)
        .env("HOME", &home)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("status json");
    assert_eq!(v["roots"][0]["collector_running"], false, "{v}");
    assert!(v["roots"][0]["checkpoint"]["stopped_at"].is_u64(), "{v}");
    std::mem::forget(c2);
}

#[test]
fn a_second_collector_for_the_same_root_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let (root, store, home) = (base.join("src"), base.join("store"), base.join("home"));
    std::fs::create_dir_all(&home).unwrap();
    tree(&root);
    let c1 = start_collector(&root, &store, &home);
    wait_for_checkpoint(&store, &root, c1.0.id());
    let out = Command::new(bin())
        .arg("collect")
        .arg(&root)
        .env("SWAMP_DIR", &store)
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("already running"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
