//! #168: declared source roots. `add-root` / `remove-root` edit the user's
//! `config.toml` in place, validate what they are given, and the first-run
//! question never blocks, never re-asks and never goes looking.
//!
//! Every test names the tempting wrong implementation it fails. Disposable
//! `tempfile` fixtures; the real config and store are never touched.

use std::fs;
use std::path::PathBuf;

use swamp_core::coverage::{RegionStatus, RootCoverage};
use swamp_core::fs_gate::StoreDir;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::roots::{
    AddOutcome, DeclaredState, FirstRun, Reach, RootError, add_root, declared_entries,
    declared_roots, first_run, remove_root, render_declared_roots,
};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: StoreDir,
    config: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let store_dir = home.join("store");
    fs::create_dir_all(&store_dir).unwrap();
    Fx {
        config: store_dir.join("config.toml"),
        store: StoreDir::at(&store_dir).unwrap(),
        home,
        _tmp: tmp,
    }
}

impl Fx {
    fn reach(&self) -> Reach {
        Reach {
            home: self.home.clone(),
            cwd: self.home.clone(),
        }
    }
    fn dir(&self, rel: &str) -> PathBuf {
        let p = self.home.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn add(&self, typed: &str) -> Result<AddOutcome, RootError> {
        add_root(&self.store, &self.reach(), typed, false)
    }
    fn text(&self) -> String {
        fs::read_to_string(&self.config).unwrap_or_default()
    }
}

const COMMENTED: &str = "\
# my swamp settings -- hand written
since = \"3d\"   # how far back growth looks
retention_days = 45

# scan scope
[scan]
defaults = true
# roots I care about
include = [
  \"~/keep-me\",   # the main one
]
exclude = [\"~/keep-me/scratch\"]
disabled_detectors = []

# other tables the user keeps
[custom]
note = \"leave alone\"
";

#[test]
fn add_root_keeps_every_comment_key_and_the_order_of_the_file() {
    // Tempting wrong patch: parse into `GrowthConfig`/`toml::Value` and
    // serialize it back, which drops every comment, reorders keys and
    // deletes the `[custom]` table swamp does not know.
    let f = fx();
    f.dir("keep-me");
    fs::write(&f.config, COMMENTED).unwrap();
    f.dir("new-root");
    let out = f.add("~/new-root").unwrap();
    assert!(matches!(out, AddOutcome::Added { .. }), "{out:?}");
    let after = f.text();
    for line in COMMENTED
        .lines()
        .filter(|l| !l.contains("include") && !l.contains("~/keep-me\","))
    {
        assert!(after.contains(line), "lost line {line:?} in:\n{after}");
    }
    assert!(after.contains("# roots I care about"));
    assert!(
        after.contains("\"~/keep-me\",   # the main one\n"),
        "comment stays on its entry:\n{after}"
    );
    assert!(
        after.contains("\n  \"~/new-root\",\n]"),
        "new entry on its own line:\n{after}"
    );
    assert!(after.contains("\"~/new-root\""));
    let entries = declared_entries(&f.store).unwrap();
    assert_eq!(entries, ["~/keep-me", "~/new-root"]);
    // Order of sections is unchanged.
    let scan = after.find("[scan]").unwrap();
    let custom = after.find("[custom]").unwrap();
    assert!(scan < custom);
}

#[test]
fn a_config_without_a_scan_table_gains_one_and_nothing_else_changes() {
    let f = fx();
    let original = "# just my notes\nsince = \"3d\"\n";
    fs::write(&f.config, original).unwrap();
    f.dir("src");
    f.add("~/src").unwrap();
    let after = f.text();
    assert!(after.starts_with(original), "{after}");
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/src"]);
}

#[test]
fn a_missing_config_file_is_created() {
    let f = fx();
    f.dir("src");
    assert!(!f.config.exists());
    f.add("~/src").unwrap();
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/src"]);
}

#[test]
fn the_same_root_spelled_any_other_way_is_a_no_op() {
    // Tempting wrong patch: compare the typed strings, so a trailing
    // slash, `..`, a relative path, `~` or a symlink each add a duplicate.
    let f = fx();
    let real = f.dir("code/real");
    f.add(real.to_str().unwrap()).unwrap();
    let before = f.text();
    std::os::unix::fs::symlink(&real, f.home.join("link")).unwrap();
    for spelling in [
        format!("{}/", real.display()),
        format!("{}/../real", real.display()),
        format!("{}/./", real.display()),
        "code/real".to_string(),
        "./code/real/".to_string(),
        "~/code/real".to_string(),
        f.home.join("link").display().to_string(),
    ] {
        let out = f.add(&spelling).unwrap();
        assert!(
            matches!(out, AddOutcome::AlreadyDeclared { .. }),
            "{spelling}: {out:?}"
        );
        assert_eq!(f.text(), before, "{spelling} must not touch the file");
    }
}

#[test]
fn what_the_user_typed_is_what_is_stored_not_the_resolved_path() {
    // Tempting wrong patch: store `fs::canonicalize` of the input, which
    // turns `~/link` into some other machine's-shaped absolute path.
    let f = fx();
    let real = f.dir("real");
    std::os::unix::fs::symlink(&real, f.home.join("link")).unwrap();
    f.add("~/link/").unwrap();
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/link"]);
    // `real` is the symlink's target, so it is the same root.
    assert!(matches!(
        f.add("real/../real"),
        Ok(AddOutcome::AlreadyDeclared { .. })
    ));
}

#[test]
fn a_root_inside_a_declared_one_is_refused_and_names_the_declared_one() {
    // Tempting wrong patch: only de-duplicate exact matches, leaving a
    // nested-redundant root that is walked twice.
    let f = fx();
    f.dir("src/app");
    f.add("~/src").unwrap();
    let before = f.text();
    let err = f.add("~/src/app").unwrap_err();
    match &err {
        RootError::NestedUnder { declared, .. } => assert_eq!(declared, "~/src"),
        other => panic!("{other:?}"),
    }
    assert!(err.to_string().contains("~/src"));
    assert_eq!(f.text(), before);
    // A sibling whose name merely starts with the declared one is not nested.
    f.dir("src2");
    assert!(matches!(f.add("~/src2"), Ok(AddOutcome::Added { .. })));
}

#[test]
fn a_parent_of_declared_roots_absorbs_them_and_says_so() {
    // Tempting wrong patch: append the parent and leave the children,
    // which stores nested-redundant roots.
    let f = fx();
    f.dir("work/a");
    f.dir("work/b");
    f.dir("other");
    f.add("~/work/a").unwrap();
    f.add("~/other").unwrap();
    f.add("~/work/b").unwrap();
    match f.add("~/work").unwrap() {
        AddOutcome::Added { stored, absorbed } => {
            assert_eq!(stored, "~/work");
            assert_eq!(absorbed, ["~/work/a", "~/work/b"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/other", "~/work"]);
}

#[test]
fn a_nonexistent_root_is_refused_unless_allow_missing_and_then_is_a_missing_root() {
    // Tempting wrong patch: record anything typed, or create the directory.
    let f = fx();
    let err = f.add("~/not-mounted").unwrap_err();
    assert!(matches!(err, RootError::Missing(_)), "{err:?}");
    assert!(err.to_string().contains("--allow-missing"));
    assert!(!f.config.exists(), "a refused add leaves no file behind");
    assert!(!f.home.join("not-mounted").exists(), "never created");

    let out = add_root(&f.store, &f.reach(), "~/not-mounted", true).unwrap();
    assert!(matches!(out, AddOutcome::Added { .. }));
    // A coverage fact, not an error: scope resolution reports it missing.
    let env = Environment::fixture(f.home.clone(), Default::default(), Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        include: declared_entries(&f.store).unwrap(),
        ..ScanConfig::default()
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1);
    let declared = declared_roots(&scope, &[]);
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].state, DeclaredState::Missing);
    let text = render_declared_roots(&declared);
    assert!(
        text.contains("missing") && text.contains("not measured"),
        "{text}"
    );
    assert!(!text.contains("0 B") && !text.contains("0B"), "{text}");
}

#[test]
fn a_file_the_filesystem_root_and_an_unreadable_directory_are_refused() {
    // Tempting wrong patch: `exists()` is the whole check.
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    fs::write(f.home.join("plain-file"), b"x").unwrap();
    assert!(matches!(
        f.add("~/plain-file"),
        Err(RootError::NotADirectory(_))
    ));
    assert!(matches!(f.add("/"), Err(RootError::TooBroad(_))));
    assert!(matches!(f.add("/../"), Err(RootError::TooBroad(_))));
    let locked = f.dir("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let got = f.add("~/locked");
    // A superuser can list anything; the refusal is the point for everyone else.
    let bypassed = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if !bypassed {
        assert!(matches!(got, Err(RootError::Unreadable { .. })), "{got:?}");
    }
    assert!(!f.config.exists());
}

#[test]
fn remove_root_of_an_undeclared_root_is_an_error_and_changes_nothing() {
    // Tempting wrong patch: rewrite the file (or report success) whether
    // or not the entry was there.
    let f = fx();
    f.dir("keep-me");
    fs::write(&f.config, COMMENTED).unwrap();
    let err = remove_root(&f.store, &f.reach(), "~/never-declared").unwrap_err();
    assert!(matches!(err, RootError::NotDeclared(_)));
    assert_eq!(f.text(), COMMENTED, "byte for byte");
}

#[test]
fn remove_root_matches_any_spelling_and_keeps_the_rest_of_the_file() {
    let f = fx();
    f.dir("keep-me");
    fs::write(&f.config, COMMENTED).unwrap();
    let removed = remove_root(&f.store, &f.reach(), "keep-me/").unwrap();
    assert_eq!(removed, "~/keep-me");
    let after = f.text();
    for line in COMMENTED
        .lines()
        .filter(|l| !l.contains("~/keep-me\"") && !l.contains("include"))
    {
        assert!(after.contains(line), "lost {line:?}:\n{after}");
    }
    assert!(declared_entries(&f.store).unwrap().is_empty());
}

#[test]
fn a_config_that_is_not_toml_or_has_the_wrong_shape_is_never_overwritten() {
    // Tempting wrong patch: treat a parse failure as "empty config" and
    // write a fresh file over the user's.
    let f = fx();
    f.dir("src");
    for broken in [
        "this is = = not toml\n",
        "[scan]\ninclude = \"~/x\"\n",
        "scan = 3\n",
    ] {
        fs::write(&f.config, broken).unwrap();
        let err = f.add("~/src").unwrap_err();
        assert!(matches!(err, RootError::Config(_)), "{broken}: {err:?}");
        assert_eq!(f.text(), broken);
    }
}

#[test]
fn a_write_that_fails_leaves_the_old_file_and_a_stale_temp_file_is_harmless() {
    // Tempting wrong patch: truncate-and-write `config.toml` in place, so a
    // crash mid-write leaves half a file. The write is a temp file plus a
    // rename; a failed write, or a temp file a crash left behind, never
    // reaches the real file.
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    f.dir("a");
    f.dir("b");
    f.add("~/a").unwrap();
    let before = f.text();
    // A crash after the temp file was written and before the rename.
    fs::write(
        f.config
            .parent()
            .unwrap()
            .join(".config.toml.tmp-1-crashed"),
        "half a fi",
    )
    .unwrap();
    assert_eq!(f.text(), before);
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/a"]);
    // A write that cannot be made (read-only store directory).
    let dir = f.config.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let got = f.add("~/b");
    let bypassed = fs::write(dir.join("probe"), b"x").is_ok();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if !bypassed {
        assert!(matches!(got, Err(RootError::Io(_))), "{got:?}");
    }
    assert_eq!(f.text(), before, "the old file is intact");
    // And the next add works.
    f.add("~/b").unwrap();
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/a", "~/b"]);
}

#[test]
fn concurrent_add_roots_all_land() {
    // Tempting wrong patch: read, edit, write with no lock, so the last
    // writer clobbers the others' entries.
    let f = fx();
    let n = 16;
    for i in 0..n {
        f.dir(&format!("r{i}"));
    }
    let store_path = f.store.path().to_path_buf();
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let store_path = store_path.clone();
            let reach = f.reach();
            std::thread::spawn(move || {
                let store = StoreDir::at(&store_path).unwrap();
                add_root(&store, &reach, &format!("~/r{i}"), false).unwrap()
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let mut got = declared_entries(&f.store).unwrap();
    got.sort();
    let mut want: Vec<String> = (0..n).map(|i| format!("~/r{i}")).collect();
    want.sort();
    assert_eq!(got, want);
}

// --- first run ---------------------------------------------------------

fn run_first_run(f: &Fx, interactive: bool, input: &str) -> (FirstRun, String) {
    let mut out = Vec::new();
    let mut lines = input.split_inclusive('\n').map(str::to_string);
    let r = first_run(
        &f.store,
        &f.reach(),
        interactive,
        &mut || lines.next(),
        &mut out,
    )
    .unwrap();
    (r, String::from_utf8(out).unwrap())
}

#[test]
fn first_run_offers_src_only_when_it_exists_and_records_the_answer() {
    let f = fx();
    f.dir("src");
    let (r, out) = run_first_run(&f, true, "\n\n");
    assert_eq!(r, FirstRun::Answered(vec!["~/src".to_string()]), "{out}");
    assert!(out.contains("Where is your source code? Press Enter for ~/src"));
    assert_eq!(declared_entries(&f.store).unwrap(), ["~/src"]);
}

#[test]
fn first_run_does_not_offer_or_propose_anything_it_did_not_stat() {
    // Tempting wrong patch: list `~`, or look for `~/code`, `~/projects`,
    // `~/dev` and offer whatever exists.
    let f = fx();
    f.dir("code");
    f.dir("projects");
    f.dir("dev");
    let (r, out) = run_first_run(&f, true, "\n");
    assert_eq!(r, FirstRun::Answered(vec![]));
    assert!(
        !out.contains("~/src"),
        "no ~/src, so none is offered: {out}"
    );
    for guess in ["~/code", "~/projects", "~/dev", "/code", "/projects"] {
        assert!(!out.contains(guess), "proposed {guess}: {out}");
    }
}

#[test]
fn first_run_asked_once_a_skip_is_remembered() {
    // Tempting wrong patch: ask again on every run until something is
    // recorded.
    let f = fx();
    let (r, _) = run_first_run(&f, true, "\n");
    assert_eq!(r, FirstRun::Answered(vec![]));
    assert!(f.text().contains("[scan]"));
    let (again, out) = run_first_run(&f, true, "\n");
    assert_eq!(again, FirstRun::NotNeeded);
    assert!(out.is_empty(), "{out}");
}

#[test]
fn first_run_does_not_re_prompt_after_an_answer() {
    let f = fx();
    f.dir("code");
    let (r, _) = run_first_run(&f, true, "~/code\n\n");
    assert_eq!(r, FirstRun::Answered(vec!["~/code".to_string()]));
    let (again, out) = run_first_run(&f, true, "");
    assert_eq!(again, FirstRun::NotNeeded);
    assert!(out.is_empty());
}

#[test]
fn first_run_never_blocks_or_writes_without_a_terminal() {
    // Tempting wrong patch: read stdin regardless of a TTY, which hangs a
    // scheduled `swamp observe` forever.
    let f = fx();
    let mut out = Vec::new();
    let r = first_run(
        &f.store,
        &f.reach(),
        false,
        &mut || panic!("read stdin without a terminal"),
        &mut out,
    )
    .unwrap();
    assert_eq!(r, FirstRun::Instructed);
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("swamp config add-root <path>"), "{out}");
    assert_eq!(out.lines().count(), 1, "one line: {out}");
    assert!(!f.config.exists(), "nothing recorded without an answer");
}

#[test]
fn first_run_is_not_asked_when_an_index_exists_or_a_scan_section_does() {
    // Tempting wrong patch: ask whenever `~/src` is not declared.
    let f = fx();
    f.dir("src");
    f.store.mark_current_format().unwrap();
    let (r, out) = run_first_run(&f, true, "\n");
    assert_eq!(r, FirstRun::NotNeeded);
    assert!(out.is_empty());

    let g = fx();
    g.dir("src");
    fs::write(&g.config, "[scan]\ndefaults = true\n").unwrap();
    let (r, out) = run_first_run(&g, true, "\n");
    assert_eq!(r, FirstRun::NotNeeded);
    assert!(out.is_empty());
    assert!(declared_entries(&g.store).unwrap().is_empty());
}

#[test]
fn first_run_reasks_after_a_bad_answer_and_gives_up_after_three() {
    let f = fx();
    let (r, out) = run_first_run(&f, true, "~/nope1\n~/nope2\n~/nope3\n~/never-read\n");
    assert_eq!(r, FirstRun::Answered(vec![]));
    assert_eq!(out.matches("does not exist").count(), 3, "{out}");
    assert!(f.text().contains("[scan]"));
}

// --- status ------------------------------------------------------------

#[test]
fn declared_root_status_takes_bytes_from_the_stored_observation_and_walks_nothing() {
    // Tempting wrong patch: size each declared root with a fresh walk (or
    // print 0 when there is no observation).
    let f = fx();
    let present = f.dir("present");
    fs::write(present.join("big"), vec![1u8; 4096]).unwrap();
    let env = Environment::fixture(f.home.clone(), Default::default(), Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        include: vec!["~/present".into(), "~/gone".into()],
        ..ScanConfig::default()
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1);
    let coverage = vec![RootCoverage {
        path: present.clone(),
        status: RegionStatus::Complete,
        walked_total: 123_456_789,
        projects: 1,
        mode: "full".into(),
        reached_by_registry: Vec::new(),
    }];
    let ((declared, text), counted) = swamp_core::work_counters::measured(|| {
        let d = declared_roots(&scope, &coverage);
        let t = render_declared_roots(&d);
        (d, t)
    });
    assert_eq!(counted.dirs_listed, 0, "no directory was listed");
    assert_eq!(counted.files_statted, 0);
    assert_eq!(declared.len(), 2);
    assert!(
        text.contains("present") && text.contains("123.5MB"),
        "{text}"
    );
    assert!(text.contains("missing"), "{text}");
    // No observation yet: not measured, never zero.
    let none = render_declared_roots(&declared_roots(&scope, &[]));
    assert!(none.contains("not measured yet"), "{none}");
    assert!(!none.contains(" 0"), "{none}");
}

#[test]
fn every_array_shape_keeps_valid_toml_and_comments_through_add_and_remove() {
    // Tempting wrong patch: `array.push` on a multi-line array, which puts
    // the new entry after a trailing comment or a comma on a line of its own.
    for original in [
        "[scan]\ninclude = [\n  \"~/a\",   # first\n]\n",
        "[scan]\ninclude = [\n  \"~/a\"   # first\n]\n",
        "[scan]\ninclude = [\n  \"~/a\",\n  \"~/b\", # two\n]\n",
        "[scan]\ninclude = [\"~/a\", \"~/b\"]\n",
        "[scan]\ninclude = []\n",
        "[scan]\ninclude = [\n]\n",
    ] {
        let f = fx();
        f.dir("new");
        fs::write(&f.config, original).unwrap();
        let before = declared_entries(&f.store).unwrap();
        f.add("~/new").unwrap();
        let after = f.text();
        after
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_else(|e| panic!("{after}\n{e}"));
        let mut want = before.clone();
        want.push("~/new".to_string());
        assert_eq!(
            declared_entries(&f.store).unwrap(),
            want,
            "{original:?} -> {after}"
        );
        for comment in ["# first", "# two"] {
            if original.contains(comment) {
                assert!(after.contains(comment), "{original:?} -> {after}");
            }
        }
        remove_root(&f.store, &f.reach(), "~/new").unwrap();
        let back = f.text();
        assert!(!back.contains("[ \""), "{back}");
        back.parse::<toml_edit::DocumentMut>()
            .unwrap_or_else(|e| panic!("{back}\n{e}"));
        assert_eq!(
            declared_entries(&f.store).unwrap(),
            before,
            "{original:?} -> {back}"
        );
    }
}

#[test]
fn removing_the_first_entry_of_a_one_line_array_leaves_no_stray_space() {
    let f = fx();
    f.dir("a");
    f.dir("b");
    fs::write(&f.config, "[scan]\ninclude = [\"~/a\", \"~/b\"]\n").unwrap();
    remove_root(&f.store, &f.reach(), "~/a").unwrap();
    assert_eq!(f.text(), "[scan]\ninclude = [\"~/b\"]\n");
}
