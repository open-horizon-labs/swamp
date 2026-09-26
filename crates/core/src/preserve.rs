//! `--keep-executables`: copy the compiled outputs a build directory
//! holds to `<worktree>/bin/` before the directory is trashed, the way
//! clean-dev-dirs' `--keep-executables` does. Every copy goes through
//! [`crate::fs_gate::destroy::copy_preserved`].

use crate::fs_gate::{self, Metadata, PermissionsExt};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A file preserved by [`preserve_executables`]: where it was, where it went.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preserved {
    pub from: PathBuf,
    pub to: PathBuf,
}

fn is_executable_file(meta: &Metadata) -> bool {
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

fn copy_into(
    from: &Path,
    dest_dir: &Path,
    sub: Option<&str>,
    out: &mut Vec<Preserved>,
) -> Result<()> {
    let to = fs_gate::destroy::copy_preserved(from, dest_dir, sub)?;
    out.push(Preserved {
        from: from.to_path_buf(),
        to,
    });
    Ok(())
}

/// Copies the compiled outputs a build directory holds to
/// `<worktree>/bin/` before the directory is trashed:
///
/// - Rust `target/`: executables (mode +x, not `.d`/`.rlib`/`.rmeta`/
///   `.dylib`/`.so`/`.a`/`.pdb`) directly in `target/release/` and
///   `target/debug/` go to `bin/release/` and `bin/debug/`.
/// - Python `dist/`: `*.whl` (and `*.tar.gz`) go to `bin/`; `build/`:
///   `*.so`/`*.pyd` anywhere inside go to `bin/`.
/// - Everything else (dependency trees, caches, other build outputs) is a
///   no-op: nothing in them is an output worth keeping.
///
/// Returns what was copied. An empty list is a valid answer, never an error.
/// `unit_path` is the build directory about to be trashed; `worktree_bin`
/// is the destination `bin/` directory to copy into.
pub fn preserve_executables(unit_path: &Path, worktree_bin: &Path) -> Result<Vec<Preserved>> {
    let mut out = Vec::new();
    let base = unit_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    const SKIP_EXT: &[&str] = &["d", "rlib", "rmeta", "a", "so", "dylib", "dll", "pdb"];
    match base {
        "target" => {
            for profile in ["release", "debug"] {
                let dir = unit_path.join(profile);
                let Ok(rd) = fs_gate::read_dir(&dir) else {
                    continue;
                };
                for e in rd.flatten() {
                    let Ok(meta) = e.metadata() else { continue };
                    let p = e.path();
                    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                    if is_executable_file(&meta) && !SKIP_EXT.contains(&ext) {
                        copy_into(&p, worktree_bin, Some(profile), &mut out)?;
                    }
                }
            }
        }
        "dist" => {
            let Ok(rd) = fs_gate::read_dir(unit_path) else {
                return Ok(out);
            };
            for e in rd.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.ends_with(".whl") || name.ends_with(".tar.gz") {
                    copy_into(&p, worktree_bin, None, &mut out)?;
                }
            }
        }
        "build" => {
            let mut stack = vec![unit_path.to_path_buf()];
            let mut seen = 0usize;
            while let Some(d) = stack.pop() {
                let Ok(rd) = fs_gate::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    seen += 1;
                    if seen > 200_000 {
                        return Ok(out);
                    }
                    let p = e.path();
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        stack.push(p);
                    } else if ft.is_file() {
                        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                        if ext == "so" || ext == "pyd" {
                            copy_into(&p, worktree_bin, None, &mut out)?;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(out)
}
