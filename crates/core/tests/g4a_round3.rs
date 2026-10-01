//! v0.8.0 G4a round 3: parsers never panic on user data, and the class a
//! consequence text earns is pinned against every text in the catalog.

use swamp_core::locations::RegenClass::{self, *};
use swamp_core::locations::SubjectShape;
use swamp_core::manager_facts::{
    parse_brew_autoremove, parse_mise_global, parse_mise_prune, parse_name_lines, parse_toolchain,
    subject_matches,
};
use swamp_core::reclaim::class_from_consequence;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const PIECES: &[&str] = &[
    "nightly",
    "stable",
    "beta",
    "1.90",
    "1.90.0",
    "-",
    "-",
    "2024-01-01",
    "2024-01-0",
    "aarch64-apple-darwin",
    "\u{e9}",
    "\u{1F600}",
    "\u{0}",
    "\u{2014}",
    "==> ",
    "Would autoremove 2 unneeded formulae:",
    "\n",
    "\r\n",
    "mise ",
    " is prunable: ",
    "@",
    ":",
    "/",
    "[",
    "]",
    "npm:@scope/pkg",
    "user/tap/foo",
    "{",
    "}",
    "\"",
    "[tools]",
    "default_toolchain = ",
    "[overrides]",
    "x",
    "0",
    "Z",
    "\u{301}",
    "\u{200b}",
    "\u{10FFFF}",
    " ",
];

fn random_string(rng: &mut Rng) -> String {
    let n = (rng.next() % 12) as usize;
    let mut s = String::new();
    for _ in 0..n {
        if rng.next().is_multiple_of(5) {
            let c = char::from_u32((rng.next() % 0x2_0000) as u32).unwrap_or('?');
            s.push(c);
        } else {
            s.push_str(PIECES[(rng.next() as usize) % PIECES.len()]);
        }
    }
    s
}

/// The tempting wrong patch: a fixed byte offset slice (`r[8..10]`) in a
/// parser over a name a person or a manager supplied. Every parser, over
/// tens of thousands of strings mixing multi-byte characters, NULs,
/// dashes, arrows and half-formed dates and lines, must return, not panic.
#[test]
fn no_parser_panics_on_arbitrary_unicode() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..40_000 {
        let s = random_string(&mut rng);
        let t = random_string(&mut rng);
        let _ = parse_toolchain(&s);
        for shape in [
            SubjectShape::FolderName,
            SubjectShape::NameBeforeAt,
            SubjectShape::ChannelWithHostTriple,
        ] {
            let _ = subject_matches(shape, &s, &t);
        }
        let bytes = s.as_bytes();
        let _ = parse_brew_autoremove(bytes);
        let _ = parse_name_lines(bytes);
        let _ = parse_mise_prune(bytes);
        let _ = parse_mise_global(bytes);
        let _ = class_from_consequence(&s);
        // Raw bytes, not valid text.
        let raw: Vec<u8> = (0..(rng.next() % 40)).map(|_| rng.next() as u8).collect();
        let _ = parse_brew_autoremove(&raw);
        let _ = parse_name_lines(&raw);
        let _ = parse_mise_prune(&raw);
        let _ = parse_mise_global(&raw);
    }
}

/// Settings values (a default, an override key and value) through the
/// real probe: no panic, whatever they hold.
#[test]
fn settings_values_of_any_shape_do_not_panic_the_probe() {
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;
    use swamp_core::external::ExternalUnit;
    use swamp_core::fs_gate::spawn::{ManagerCommand, RunOutput};
    use swamp_core::last_used::LastUsed;
    use swamp_core::locations::{Provenance, StorageCategory};
    use swamp_core::manager_facts::{ProbeRunner, collect_within};
    struct R(String);
    impl ProbeRunner for R {
        fn run(&self, _c: ManagerCommand, _t: Duration) -> io::Result<RunOutput> {
            Err(io::Error::new(io::ErrorKind::NotFound, "x"))
        }
        fn read_settings(&self, _p: &std::path::Path) -> Result<String, String> {
            Ok(self.0.clone())
        }
    }
    let unit = ExternalUnit {
        detector_id: "rustup".into(),
        detector_name: "rustup".into(),
        category: StorageCategory::LocalState,
        provenance: Provenance::BuiltinConvention,
        path: PathBuf::from("/h/.rustup"),
        bytes: 1,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children: Vec::new(),
    };
    let toolchains = ExternalUnit {
        category: StorageCategory::Installation,
        path: PathBuf::from("/h/.rustup/toolchains"),
        ..unit.clone()
    };
    let units = [unit, toolchains];
    let mut rng = Rng(42);
    for _ in 0..3_000 {
        let a = random_string(&mut rng);
        let b = random_string(&mut rng);
        let text = format!(
            "default_toolchain = \"{}\"\n[overrides]\n\"{}\" = \"{}\"\n",
            a.replace(['"', '\\', '\n', '\r', '\u{0}'], ""),
            b.replace(['"', '\\', '\n', '\r', '\u{0}'], ""),
            a.replace(['"', '\\', '\n', '\r', '\u{0}'], "")
        );
        let _ = collect_within(
            &units,
            &R(text),
            1,
            Duration::from_secs(5),
            Duration::from_secs(1),
        );
        let _ = collect_within(
            &units,
            &R(a),
            1,
            Duration::from_secs(5),
            Duration::from_secs(1),
        );
    }
}

/// What a person picks for each consequence text a build adapter can
/// state (`class_from_consequence` is the same choice, mechanically). The
/// catalog check below makes a new text without an entry here fail.
const TABLE: &[(&str, RegenClass)] = &[
    (
        "the next Gradle build of this module regenerates it",
        Rebuild,
    ),
    (
        "the next native build reconfigures and recompiles (a slower build)",
        Rebuild,
    ),
    (
        "reinstall with `sdkmanager` (or Android Studio's SDK Manager): a download",
        Download,
    ),
    ("the next assemble of this variant builds it again", Rebuild),
    (
        "the next bundle task for this variant builds it again",
        Rebuild,
    ),
    (
        "the next `cargo build` recompiles these dependencies from the already-downloaded sources",
        Rebuild,
    ),
    (
        "the next `cargo build` in this profile is a full rebuild rather than an incremental one",
        Rebuild,
    ),
    (
        "the next `cargo build` re-runs this crate's build script",
        Rebuild,
    ),
    ("the next `cargo test` relinks this test binary", Rebuild),
    (
        "the next `cargo build --examples` relinks this example",
        Rebuild,
    ),
    ("the next `cargo build` relinks this output", Rebuild),
    (
        "Cargo rewrites this metadata on the next build of the target it describes",
        Rebuild,
    ),
    (
        "the next `cargo build` in this profile rebuilds everything it holds",
        Rebuild,
    ),
    (
        "the next `cargo build` regenerates what it needs from source",
        Rebuild,
    ),
    (
        "the next build that reaches this step runs it again",
        Rebuild,
    ),
    ("the next build uploads its context again", Rebuild),
    ("the next build clones the repository again", Download),
    ("rebuild with this project's build", Rebuild),
    (
        "the next build downloads the modules it needs again -- needs module proxy access",
        Download,
    ),
    (
        "the next build with this Gradle version re-derives it",
        Rebuild,
    ),
    (
        "the next build re-downloads modules and distributions it needs -- needs network access",
        Download,
    ),
    (
        "the next build re-resolves module metadata -- needs network access",
        Download,
    ),
    (
        "the next build downloads this module again -- needs network access",
        Download,
    ),
    (
        "the next Gradle daemon build starts a new daemon and a new log",
        Rebuild,
    ),
    (
        "the next `mvn package` regenerates everything it holds",
        Rebuild,
    ),
    ("the next `mvn package` repackages it", Rebuild),
    (
        "rebuild with this project's build script (commonly `npm run build`)",
        Rebuild,
    ),
    ("rebuild with `nuxt build`", Rebuild),
    ("a test rerun with coverage enabled regenerates it", Rebuild),
    (
        "the next task run is a cache miss and re-executes rather than replaying",
        Rebuild,
    ),
    ("the next `parcel build` is a cold build", Rebuild),
    (
        "the next dev server start re-optimizes dependencies",
        Rebuild,
    ),
    (
        "the next build recompiles what it would otherwise have replayed",
        Rebuild,
    ),
    ("the next `ng build` is a cold build", Rebuild),
    ("the next Expo start rebuilds its cache", Rebuild),
    (
        "reinstall with `npm ci` (or `pnpm install`) -- needs registry access",
        Download,
    ),
    (
        "the next install downloads this content again -- needs registry access",
        Download,
    ),
    ("rebuild with this project's build frontend", Rebuild),
    (
        "rebuild the distributions with this project's build frontend",
        Rebuild,
    ),
    (
        "the next setuptools build recompiles and recopies what it holds",
        Rebuild,
    ),
    (
        "the interpreter recompiles the bytecode on the next import",
        Rebuild,
    ),
    (
        "pytest forgets its last-failed set (`--lf`, `--ff` start from scratch)",
        NotEstablished,
    ),
    (
        "the next mypy run type-checks every module instead of the changed ones",
        Rebuild,
    ),
    (
        "the next ruff run re-lints every file instead of the changed ones",
        Rebuild,
    ),
    ("the next pytype run re-analyses every module", Rebuild),
    ("the next Pyre run rebuilds its state", Rebuild),
    (
        "Hypothesis forgets the failing examples it saved and must find them again",
        NotEstablished,
    ),
    ("a test run with `coverage html` regenerates it", Rebuild),
    (
        "tox recreates each environment on its next run -- needs index access",
        Download,
    ),
    (
        "setuptools fetches the build requirements again -- needs index access",
        Download,
    ),
    (
        "the next setuptools build or editable install rewrites it",
        Rebuild,
    ),
    ("a test run under coverage writes it again", Rebuild),
    ("the next build stages them again", Rebuild),
    ("the next build recompiles the extension modules", Rebuild),
    ("the next bdist build restages it", Rebuild),
    (
        "reinstall this distribution into the environment -- needs index access",
        Download,
    ),
    (
        "uv downloads the interpreters again when a project asks for them",
        Download,
    ),
    (
        "reinstall each tool with `uv tool install` -- needs index access",
        Download,
    ),
    (
        "recreate the environment and reinstall its packages -- needs index access, or the local wheel cache",
        Download,
    ),
    (
        "Xcode rebuilds and re-indexes each project the next time it is opened or built",
        Rebuild,
    ),
    (
        "Xcode rebuilds and re-indexes this project the next time it is built",
        Rebuild,
    ),
    (
        "the next `swift build` rebuilds it and resolves dependencies again",
        Rebuild,
    ),
    (
        "the next simulator boot rebuilds what it needs (a slower boot)",
        Rebuild,
    ),
    ("the next test build produces it again", Rebuild),
    (
        "reinstall with ESP-IDF's `install.sh` (or `idf_tools.py install`)",
        Download,
    ),
    (
        "iOS_23F77 is downloaded again when a simulator needs it",
        Download,
    ),
    ("uv unpacks these again from downloaded wheels", Download),
    (
        "this emulator's apps and data are gone; a recreated AVD starts empty",
        NotRegenerable,
    ),
    (
        "session scratch; removing it during a session breaks that session",
        NotEstablished,
    ),
    (
        "the download cache is shared by every Go build on this machine",
        Download,
    ),
    (
        "each repo is downloaded again from huggingface.co when a program asks for it",
        Download,
    ),
    (
        "each model is downloaded again with `ollama pull` when the registry has it",
        Download,
    ),
];

/// The tempting wrong patch: a cue list tuned on a few texts. Every text
/// in the table earns the class a person picks, and a text the adapters
/// state that is not in the table fails here (the extraction reads the
/// `.consequence("...")` literals and `*CONSEQUENCE*` constants of the
/// adapters' sources, so a new one cannot be added unclassified).
#[test]
fn every_catalog_consequence_earns_the_class_a_person_picks() {
    for (text, want) in TABLE {
        assert_eq!(class_from_consequence(text), *want, "{text}");
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/build_adapters");
    let mut literals: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let src = src.split("#[cfg(test)]").next().unwrap();
        let mut rest = src;
        while let Some(i) = rest.find(".consequence(\"") {
            let after = &rest[i + ".consequence(\"".len()..];
            let end = after.find('"').unwrap();
            literals.push(after[..end].replace("\\'", "'"));
            rest = &after[end..];
        }
    }
    assert!(
        literals.len() > 15,
        "the extraction found {} texts",
        literals.len()
    );
    let missing: Vec<&String> = literals
        .iter()
        // A format string: its parts are pinned above.
        .filter(|l| !l.contains('{'))
        .filter(|l| !TABLE.iter().any(|(t, _)| t == l))
        .collect();
    assert!(
        missing.is_empty(),
        "the adapters state consequences with no expected class: {missing:#?}"
    );
}
