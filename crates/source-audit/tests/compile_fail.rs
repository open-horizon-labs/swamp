//! The type-level half of the guardrails: every shortcut a retired
//! semantic audit used to look for, written against swamp-core's
//! production API, must fail to compile with the expected error.
//!
//! The cases live in `crates/core/tests/compile_fail/` (each with its
//! expected `.stderr`; guardrail frontmatter names them under
//! `compile_fail:`), and run from this crate so they build against
//! swamp-core *without* its `testing` feature. Regenerate the expected
//! output after an intended API change with `TRYBUILD=overwrite`.
//!
//! A rustc diagnostic can depend on which *other* crates a platform
//! pulls in, not only on rustc's own version: `core-foundation` (a
//! macOS-only dependency, via FSEvents) gives `bool` a second `From`
//! impl candidate that Linux's dependency graph never has, so
//! `occupancy_does_not_convert_to_bool`'s "which other types implement
//! this trait" suggestion is genuinely different text per OS, not a
//! toolchain-version drift `TRYBUILD=overwrite` should paper over.
//! `<case>.stderr.<target_os>` (`linux`, `macos`, ...) is this file's
//! escape hatch for exactly that: found next to a case, it is swapped
//! in for the run and the original `.stderr` restored afterward
//! (`OsStderrOverrides`'s `Drop`), so the checked-in default stays
//! whatever OS regenerated it last and no OS's expectation is ever
//! silently overwritten by another's.

use std::fs;
use std::path::{Path, PathBuf};

const CASES_DIR: &str = "../core/tests/compile_fail";

/// Backs up every `<case>.stderr` this run is about to overwrite with an
/// OS-specific `<case>.stderr.<target_os>`, and puts each one back
/// (or removes it, if there was none) when dropped -- including on a
/// panic, so a failed run never leaves another OS's expectation
/// checked out.
struct OsStderrOverrides {
    // (the real `.stderr` path, its original content if any present)
    restore: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl OsStderrOverrides {
    fn install(dir: &Path) -> std::io::Result<Self> {
        let os = std::env::consts::OS;
        let suffix = format!(".stderr.{os}");
        let mut restore = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(&suffix) else {
                continue;
            };
            let override_content = fs::read(&path)?;
            let target = dir.join(format!("{stem}.stderr"));
            let original = fs::read(&target).ok();
            fs::write(&target, &override_content)?;
            restore.push((target, original));
        }
        Ok(Self { restore })
    }
}

impl Drop for OsStderrOverrides {
    fn drop(&mut self) {
        for (path, original) in &self.restore {
            match original {
                Some(content) => {
                    let _ = fs::write(path, content);
                }
                None => {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

#[test]
#[ignore = "heavy harness: scripts/check-full.sh runs it once, with --ignored"]
fn retired_rule_shortcuts_do_not_compile() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(CASES_DIR);
    let _overrides = OsStderrOverrides::install(&dir).expect("swap in this OS's .stderr overrides");
    let t = trybuild::TestCases::new();
    t.compile_fail("../core/tests/compile_fail/*.rs");
}
