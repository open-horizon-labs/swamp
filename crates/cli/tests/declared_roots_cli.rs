//! #168 through the real binary: `swamp config add-root` / `remove-root`
//! against a scratch store (never the maintainer's config), what `config
//! show` and `scope` print for declared roots, and that racing processes
//! all land. Detectors are irrelevant here; only `config` and `scope`
//! run, and neither observes anything.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn run(store: &Path, home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .env("SWAMP_DIR", store)
        .env("SWAMP_LOG_DIR", store)
        .env("SWAMP_LAUNCH_AGENTS_DIR", store)
        .env("SWAMP_TEST_MODE", "1")
        .env("HOME", home)
        .env("HOMEBREW_PREFIX", home.join("no-brew"))
        .output()
        .expect("run swamp")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(tmp.path()).unwrap();
    let store = home.join("store");
    std::fs::create_dir_all(&store).unwrap();
    Fx {
        _tmp: tmp,
        home,
        store,
    }
}

#[test]
fn add_root_edits_the_config_and_config_show_and_scope_list_it() {
    let f = fx();
    std::fs::create_dir_all(f.home.join("code")).unwrap();
    std::fs::write(f.store.join("config.toml"), "# mine\nsince = \"3d\"\n").unwrap();
    let out = run(
        &f.store,
        &f.home,
        &f.home,
        &["config", "add-root", "~/code"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let cfg = std::fs::read_to_string(f.store.join("config.toml")).unwrap();
    assert!(cfg.starts_with("# mine\nsince = \"3d\"\n"), "{cfg}");
    assert!(cfg.contains("\"~/code\""));

    let show = run(&f.store, &f.home, &f.home, &["config", "show"]);
    let t = text(&show);
    assert!(show.status.success(), "{t}");
    assert!(
        t.contains("declared source roots:") && t.contains("present"),
        "{t}"
    );
    assert!(
        t.contains(&f.home.join("code").display().to_string()),
        "{t}"
    );

    let scope = run(&f.store, &f.home, &f.home, &["scope"]);
    let t = text(&scope);
    assert!(t.contains("declared source roots:"), "{t}");
    // Never observed: not measured, not zero.
    assert!(t.contains("not measured yet"), "{t}");
}

#[test]
fn a_missing_declared_root_is_listed_missing_and_is_not_an_error() {
    let f = fx();
    let refused = run(
        &f.store,
        &f.home,
        &f.home,
        &["config", "add-root", "/Volumes/not-mounted-yet"],
    );
    assert!(!refused.status.success());
    assert!(
        text(&refused).contains("--allow-missing"),
        "{}",
        text(&refused)
    );
    let ok = run(
        &f.store,
        &f.home,
        &f.home,
        &[
            "config",
            "add-root",
            "/Volumes/not-mounted-yet",
            "--allow-missing",
        ],
    );
    assert!(ok.status.success(), "{}", text(&ok));
    let scope = run(&f.store, &f.home, &f.home, &["scope"]);
    assert!(scope.status.success(), "{}", text(&scope));
    let t = text(&scope);
    assert!(
        t.lines().any(|l| l.contains("missing")
            && l.contains("/Volumes/not-mounted-yet")
            && l.contains("not measured")),
        "{t}"
    );
}

#[test]
fn removing_an_undeclared_root_fails_and_leaves_the_file_alone() {
    let f = fx();
    let original = "# keep\n[scan]\ninclude = []\n";
    std::fs::write(f.store.join("config.toml"), original).unwrap();
    let out = run(
        &f.store,
        &f.home,
        &f.home,
        &["config", "remove-root", "~/x"],
    );
    assert!(!out.status.success());
    assert!(text(&out).contains("not a declared root"), "{}", text(&out));
    assert_eq!(
        std::fs::read_to_string(f.store.join("config.toml")).unwrap(),
        original
    );
}

#[test]
fn racing_processes_all_land() {
    // Tempting wrong patch: unlocked read-modify-write; the last process
    // to write clobbers the others.
    let f = fx();
    let n = 10;
    for i in 0..n {
        std::fs::create_dir_all(f.home.join(format!("p{i}"))).unwrap();
    }
    let children: Vec<_> = (0..n)
        .map(|i| {
            Command::new(bin())
                .args(["config", "add-root", &format!("~/p{i}")])
                .current_dir(&f.home)
                .env("SWAMP_DIR", &f.store)
                .env("SWAMP_LOG_DIR", &f.store)
                .env("SWAMP_LAUNCH_AGENTS_DIR", &f.store)
                .env("SWAMP_TEST_MODE", "1")
                .env("HOME", &f.home)
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let cfg = std::fs::read_to_string(f.store.join("config.toml")).unwrap();
    for i in 0..n {
        assert_eq!(cfg.matches(&format!("\"~/p{i}\"")).count(), 1, "{cfg}");
    }
}
