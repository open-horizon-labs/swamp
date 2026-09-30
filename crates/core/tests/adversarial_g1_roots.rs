//! Adversarial tests for #168 (declared source roots), written by the
//! v0.8.0 G1 audit. Each test names the wrong-but-plausible implementation
//! it is aimed at. A test that fails on the PR head is a finding.
//!
//! Disposable `tempfile` fixtures only; the real config and store are never
//! touched.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use swamp_core::fs_gate::StoreDir;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::roots::{
    AddOutcome, FirstRun, Reach, RootError, add_root, declared_entries, declared_roots, first_run,
    remove_root, render_declared_roots,
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
    fn entries(&self) -> Vec<String> {
        declared_entries(&self.store).unwrap()
    }
}

fn parses(text: &str) {
    text.parse::<toml_edit::DocumentMut>()
        .unwrap_or_else(|e| panic!("edited config is not TOML: {e}\n{text}"));
}

// --- config file shapes -------------------------------------------------

/// Tempting wrong patch: push new entries with a bare `\n` prefix, so a
/// CRLF file comes back with mixed line endings.
#[test]
fn crlf_config_keeps_crlf_line_endings() {
    let f = fx();
    f.dir("a");
    f.dir("b");
    fs::write(
        &f.config,
        "since = \"3d\"\r\n[scan]\r\ninclude = [\r\n  \"~/a\",\r\n]\r\n",
    )
    .unwrap();
    assert!(matches!(f.add("~/b").unwrap(), AddOutcome::Added { .. }));
    let text = f.text();
    parses(&text);
    let bare = text
        .char_indices()
        .filter(|(i, c)| *c == '\n' && (*i == 0 || text.as_bytes()[i - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "bare LF written into a CRLF file: {text:?}");
}

/// Tempting wrong patch: parse without stripping the BOM, or strip it and
/// drop it on write.
#[test]
fn bom_config_is_edited_and_keeps_its_other_keys() {
    let f = fx();
    f.dir("a");
    fs::write(&f.config, "\u{feff}since = \"3d\"\n[scan]\ninclude = []\n").unwrap();
    let r = f.add("~/a");
    assert!(matches!(r, Ok(AddOutcome::Added { .. })), "{r:?}");
    let text = f.text();
    assert!(text.contains("since = \"3d\""), "{text}");
    assert_eq!(f.entries(), vec!["~/a".to_string()]);
}

/// Tempting wrong patch: only handle `[scan]` as a standard table.
#[test]
fn inline_table_and_dotted_key_shapes_are_edited_in_place() {
    for shape in [
        "scan = { include = [\"~/a\"] }\n",
        "scan.include = [\"~/a\"]\n",
    ] {
        let f = fx();
        f.dir("a");
        f.dir("b");
        fs::write(&f.config, shape).unwrap();
        let r = f.add("~/b");
        assert!(matches!(r, Ok(AddOutcome::Added { .. })), "{shape}: {r:?}");
        let text = f.text();
        parses(&text);
        assert_eq!(
            f.entries(),
            vec!["~/a".to_string(), "~/b".to_string()],
            "{text}"
        );
        remove_root(&f.store, &f.reach(), "~/a").unwrap();
        assert_eq!(f.entries(), vec!["~/b".to_string()], "{}", f.text());
    }
}

/// Tempting wrong patch: pick the first `[scan]`, or overwrite a
/// non-array `include` with an array.
#[test]
fn duplicate_scan_tables_and_non_array_include_are_refused_untouched() {
    for bad in [
        "[scan]\ninclude = []\n[scan]\ndefaults = true\n",
        "[scan]\ninclude = \"~/a\"\n",
        "[scan]\ninclude = { a = 1 }\n",
        "scan = 3\n",
    ] {
        let f = fx();
        f.dir("b");
        fs::write(&f.config, bad).unwrap();
        let r = f.add("~/b");
        assert!(matches!(r, Err(RootError::Config(_))), "{bad}: {r:?}");
        assert_eq!(f.text(), bad);
    }
}

/// Tempting wrong patch: a line-oriented editor with a size limit.
#[test]
fn a_very_large_config_survives_an_edit_byte_for_byte_outside_include() {
    let f = fx();
    f.dir("a");
    let mut big = String::from("[scan]\ninclude = []\n[custom]\n");
    for i in 0..60_000 {
        big.push_str(&format!("# comment line {i} with some padding text\n"));
    }
    fs::write(&f.config, &big).unwrap();
    f.add("~/a").unwrap();
    let text = f.text();
    assert_eq!(text.replace("include = [\"~/a\"]", "include = []"), big);
}

/// Tempting wrong patch: write a temp file and rename it over the path,
/// which replaces a symlinked config with a regular file and leaves the
/// real file (e.g. in a dotfiles repo) unchanged.
#[test]
fn a_symlinked_config_stays_a_symlink_and_the_target_is_edited() {
    let f = fx();
    f.dir("a");
    let real_dir = f.dir("dotfiles");
    let real = real_dir.join("swamp.toml");
    fs::write(&real, "# mine\n[scan]\ninclude = []\n").unwrap();
    std::os::unix::fs::symlink(&real, &f.config).unwrap();
    f.add("~/a").unwrap();
    let meta = fs::symlink_metadata(&f.config).unwrap();
    assert!(
        meta.file_type().is_symlink(),
        "config.toml was replaced by a regular file"
    );
    assert!(
        fs::read_to_string(&real).unwrap().contains("~/a"),
        "the symlink target was not edited"
    );
}

/// Tempting wrong patch: the rename writes a fresh 0644 file, widening a
/// config the user made private (0600) or ignoring a read-only one.
#[test]
fn the_config_file_mode_is_kept() {
    for mode in [0o600, 0o444] {
        let f = fx();
        f.dir("a");
        fs::write(&f.config, "[scan]\ninclude = []\n").unwrap();
        fs::set_permissions(&f.config, fs::Permissions::from_mode(mode)).unwrap();
        let r = f.add("~/a");
        let after = fs::metadata(&f.config).unwrap().permissions().mode() & 0o777;
        if mode == 0o444 {
            // Either refuse (file unchanged) or keep it read-only.
            assert!(
                r.is_err() || after == 0o444,
                "a read-only config was rewritten as {after:o}: {r:?}"
            );
        } else {
            assert_eq!(after, mode, "mode {mode:o} became {after:o}");
        }
    }
}

/// Tempting wrong patch: ignore a failed temp write in a read-only store
/// directory, or leave a half-written file.
#[test]
fn a_store_directory_that_cannot_be_written_leaves_the_config_unchanged() {
    let f = fx();
    f.dir("a");
    let before = "[scan]\ninclude = []\n";
    fs::write(&f.config, before).unwrap();
    // Take the lock file first so only the rename/temp write can fail.
    drop(f.store.lock_config_edits().unwrap());
    let dir = f.config.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let r = f.add("~/a");
    // A superuser can write into a 0555 directory; the refusal is for everyone else.
    let bypassed = fs::write(dir.join("probe"), b"x").is_ok();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if bypassed {
        return;
    }
    assert!(matches!(r, Err(RootError::Io(_))), "{r:?}");
    assert_eq!(f.text(), before);
}

// --- path spellings -----------------------------------------------------

/// Tempting wrong patch: store with lossy conversion or mangle non-ASCII.
#[test]
fn unicode_and_space_paths_round_trip() {
    let f = fx();
    f.dir("My Code/ünïcødé 源");
    let r = f.add("~/My Code/ünïcødé 源").unwrap();
    assert!(matches!(r, AddOutcome::Added { .. }));
    parses(&f.text());
    assert_eq!(f.entries(), vec!["~/My Code/ünïcødé 源".to_string()]);
    remove_root(&f.store, &f.reach(), "~/My Code/ünïcødé 源/").unwrap();
    assert!(f.entries().is_empty());
}

/// Tempting wrong patch: `home.join(rest)` after stripping `~/`, so
/// `~//src` joins an absolute `/src` and records the machine's `/src`.
#[test]
fn double_slash_after_tilde_is_the_home_directory_not_the_filesystem_root() {
    let f = fx();
    f.dir("src");
    let r = f.add("~//src");
    assert_eq!(
        r,
        Ok(AddOutcome::Added {
            stored: "~/src".into(),
            absorbed: vec![]
        })
    );
}

#[test]
fn trailing_dot_and_double_slash_spellings_are_one_root() {
    let f = fx();
    f.dir("src");
    f.add("~/src").unwrap();
    for spelling in ["~/src/.", "~/src//", "~/./src", "~/src/../src"] {
        assert!(
            matches!(f.add(spelling), Ok(AddOutcome::AlreadyDeclared { .. })),
            "{spelling}"
        );
    }
    let abs = format!("{}//src/.", f.home.display());
    assert!(matches!(
        f.add(&abs),
        Ok(AddOutcome::AlreadyDeclared { .. })
    ));
}

/// Tempting wrong patch: treat `~bob/src` as a relative path, so
/// `--allow-missing` records `<cwd>/~bob/src`.
#[test]
fn a_tilde_user_form_is_refused_not_recorded_relative_to_cwd() {
    let f = fx();
    let r = add_root(&f.store, &f.reach(), "~nobody/src", true);
    if let Ok(AddOutcome::Added { stored, .. }) = &r {
        panic!("~user form recorded as {stored:?}");
    }
}

/// Tempting wrong patch: compare byte strings, so `~/Src` and `~/src` on
/// a case-insensitive volume are two roots over the same bytes.
#[test]
fn case_variants_on_a_case_insensitive_volume_are_one_root() {
    let f = fx();
    f.dir("src");
    if !f.home.join("SRC").exists() {
        eprintln!("case-sensitive volume; nothing to check");
        return;
    }
    f.add("~/src").unwrap();
    let r = f.add("~/Src");
    assert!(
        matches!(r, Ok(AddOutcome::AlreadyDeclared { .. })),
        "{r:?}\n{}",
        f.text()
    );
}

/// Tempting wrong patch: canonicalize and trust it; macOS firmlinks
/// (`/System/Volumes/Data/...`) do not canonicalize to the `/...` spelling.
#[test]
fn a_firmlink_spelling_is_the_same_root() {
    let f = fx();
    let data = PathBuf::from("/System/Volumes/Data").join(f.home.strip_prefix("/").unwrap());
    if !data.exists() {
        eprintln!("no firmlinked data volume; nothing to check");
        return;
    }
    f.dir("src");
    f.add("~/src").unwrap();
    let r = f.add(&data.join("src").display().to_string());
    assert!(
        matches!(r, Ok(AddOutcome::AlreadyDeclared { .. })),
        "{r:?}\n{}",
        f.text()
    );
}

#[test]
fn tmp_and_private_tmp_spellings_are_one_root() {
    let f = fx();
    let name = format!("swamp-adv-g1-{}", std::process::id());
    let real = PathBuf::from("/private/tmp").join(&name);
    if !Path::new("/tmp").is_symlink() {
        return;
    }
    fs::create_dir_all(&real).unwrap();
    let a = add_root(&f.store, &f.reach(), &format!("/tmp/{name}"), false);
    let b = add_root(&f.store, &f.reach(), &real.display().to_string(), false);
    let removed = remove_root(&f.store, &f.reach(), &real.display().to_string());
    fs::remove_dir(&real).unwrap();
    assert!(matches!(a, Ok(AddOutcome::Added { .. })), "{a:?}");
    assert!(matches!(b, Ok(AddOutcome::AlreadyDeclared { .. })), "{b:?}");
    assert!(removed.is_ok(), "{removed:?}");
    assert!(f.entries().is_empty());
}

/// Tempting wrong patch: remove only the first matching entry, so a
/// hand-written file with the same root spelled twice reports "no longer
/// declared" while the root stays in scope.
#[test]
fn remove_root_leaves_no_spelling_of_the_root_declared() {
    let f = fx();
    let src = f.dir("src");
    fs::write(
        &f.config,
        format!("[scan]\ninclude = [\"~/src\", \"{}/\"]\n", src.display()),
    )
    .unwrap();
    remove_root(&f.store, &f.reach(), "~/src").unwrap();
    assert!(
        f.entries().is_empty(),
        "still declared after remove-root: {:?}",
        f.entries()
    );
}

#[test]
fn removing_a_parent_of_declared_roots_is_refused_and_changes_nothing() {
    let f = fx();
    f.dir("src/a");
    f.add("~/src/a").unwrap();
    let before = f.text();
    let r = remove_root(&f.store, &f.reach(), "~/src");
    assert!(matches!(r, Err(RootError::NotDeclared(_))), "{r:?}");
    assert_eq!(f.text(), before);
}

#[test]
fn a_missing_root_is_removed_by_a_different_spelling() {
    let f = fx();
    add_root(&f.store, &f.reach(), "~/gone/deep", true).unwrap();
    let abs = format!("{}/gone/./deep/", f.home.display());
    assert_eq!(
        remove_root(&f.store, &f.reach(), &abs).unwrap(),
        "~/gone/deep"
    );
}

/// Tempting wrong patch: a relative path joined to a non-UTF-8 cwd with
/// `to_string_lossy`, so U+FFFD is written instead of refusing.
#[test]
fn a_relative_root_under_a_non_utf8_cwd_is_refused_not_mangled() {
    use std::os::unix::ffi::OsStrExt;
    let f = fx();
    let cwd = f.home.join(std::ffi::OsStr::from_bytes(b"bad\xffname"));
    let reach = Reach {
        home: f.home.clone(),
        cwd,
    };
    let r = add_root(&f.store, &reach, "proj", true);
    assert!(
        matches!(r, Err(RootError::NotUtf8(_))),
        "{r:?}\n{}",
        f.text()
    );
}

/// Tempting wrong patch: `HOME` unset falls back to `.`, and `~/src`
/// is then recorded as the relative entry `src` (which scope resolution
/// reads relative to whatever home it later sees).
#[test]
fn with_home_unset_a_tilde_root_is_never_recorded_relative() {
    let f = fx();
    let reach = Reach {
        home: PathBuf::from("."),
        cwd: f.home.clone(),
    };
    let r = add_root(&f.store, &reach, "~/src", true);
    if let Ok(AddOutcome::Added { stored, .. }) = &r {
        assert!(
            stored.starts_with('/') || stored.starts_with("~/"),
            "recorded relative entry {stored:?}"
        );
    }
}

// --- first run ----------------------------------------------------------

fn run_first_run(f: &Fx, input: Vec<String>) -> (FirstRun, String) {
    let mut out = Vec::new();
    let mut lines = input.into_iter();
    let r = first_run(&f.store, &f.reach(), true, &mut || lines.next(), &mut out).unwrap();
    (r, String::from_utf8_lossy(&out).into_owned())
}

/// Tempting wrong patch: echo the typed answer into the error line, so
/// escape sequences typed at the prompt are replayed to the terminal.
#[test]
fn escape_sequences_typed_at_the_prompt_are_not_echoed_raw() {
    let f = fx();
    let (_, out) = run_first_run(&f, vec!["\x1b]0;pwned\x07\x1b[2J~/x\n".into()]);
    assert!(!out.contains('\x1b'), "raw ESC echoed: {out:?}");
}

#[test]
fn eof_garbage_and_huge_lines_never_panic_and_the_question_is_remembered() {
    let f = fx();
    let huge = "x".repeat(1 << 20) + "\n";
    let (r, _) = run_first_run(&f, vec![huge, "\0\0\n".into(), "\u{7f}\n".into()]);
    assert_eq!(r, FirstRun::Answered(vec![]));
    assert!(f.text().contains("[scan]"));
    // EOF right away (Ctrl-D)
    let g = fx();
    let (r, _) = run_first_run(&g, vec![]);
    assert_eq!(r, FirstRun::Answered(vec![]));
    let (r, _) = run_first_run(&g, vec![]);
    assert_eq!(r, FirstRun::NotNeeded, "asked again after Ctrl-D");
}

#[test]
fn no_src_offers_nothing_but_explains() {
    let f = fx();
    let (_, out) = run_first_run(&f, vec!["\n".into()]);
    assert!(!out.contains("~/src"), "{out}");
    assert!(out.contains("built-in defaults"), "{out}");
    assert!(out.contains("add-root"), "{out}");
}

#[test]
fn a_src_symlink_to_a_missing_target_is_not_offered() {
    let f = fx();
    std::os::unix::fs::symlink(f.home.join("nowhere"), f.home.join("src")).unwrap();
    let (_, out) = run_first_run(&f, vec!["\n".into()]);
    assert!(!out.contains("Enter for ~/src"), "{out}");
}

#[test]
fn a_skip_then_add_root_appends_to_the_empty_include() {
    let f = fx();
    f.dir("a");
    f.dir("b");
    run_first_run(&f, vec!["\n".into()]);
    f.add("~/a").unwrap();
    f.add("~/b").unwrap();
    parses(&f.text());
    assert_eq!(
        f.entries(),
        vec!["~/a".to_string(), "~/b".to_string()],
        "{}",
        f.text()
    );
}

// --- status -------------------------------------------------------------

/// Tempting wrong patch: stat or size each declared root to render status.
#[test]
fn a_thousand_declared_roots_render_without_listing_anything() {
    let f = fx();
    let include: Vec<String> = (0..1000).map(|i| format!("~/r{i}")).collect();
    for i in (0..1000).step_by(2) {
        f.dir(&format!("r{i}"));
    }
    let env = Environment::fixture(f.home.clone(), Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(
        &env,
        &ScanConfig {
            include,
            ..ScanConfig::default()
        },
        &[],
        &Registry::with_builtins(),
        1,
    );
    let (text, counted) =
        swamp_core::work_counters::measured(|| render_declared_roots(&declared_roots(&scope, &[])));
    assert_eq!(counted.dirs_listed, 0);
    assert_eq!(counted.files_statted, 0);
    assert_eq!(text.matches("missing").count(), 500, "{}", &text[..500]);
    assert_eq!(text.matches("not measured yet").count(), 500);
}

#[test]
fn nested_and_unreadable_declared_roots_are_reported_as_such() {
    let f = fx();
    f.dir("outer/inner");
    let locked = f.dir("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let env = Environment::fixture(f.home.clone(), Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(
        &env,
        &ScanConfig {
            include: vec!["~/outer".into(), "~/outer/inner".into(), "~/locked".into()],
            ..ScanConfig::default()
        },
        &[],
        &Registry::with_builtins(),
        1,
    );
    let text = render_declared_roots(&declared_roots(&scope, &[]));
    // A superuser can open a mode-000 directory; it is unreadable for everyone else.
    let bypassed = fs::read_dir(&locked).is_ok();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(text.contains("covered"), "{text}");
    if !bypassed {
        assert!(text.contains("unreadable"), "{text}");
    }
}
