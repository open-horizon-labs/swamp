//! The capability gate: the only module in `crates/{core,cli,tui}` that
//! names `std::fs`, `std::os::unix::fs`, whole-file `std::io` reads,
//! `OpenOptions`, `tempfile`, `libc`, `std::process`, the Parquet
//! writer/reader, or the FSEvents FFI (`.oh/guardrails/*`, "Detection").
//!
//! # Why a gate instead of an audit
//!
//! Four independent review rounds (2026-09-21 .. 2026-09-22) showed that
//! a `syn` call graph without type resolution cannot be made
//! mutation-proof: a fn item bound to a local, a fn pointer in a struct
//! field, a UFCS call, a glob import, a local `macro_rules!`, a
//! `#[cfg(any())]` block or an orphan file each hid a destructive call
//! from every rule. So the guardrail *semantics* moved into the type
//! system, and the audits shrank to exact path-reference rules:
//!
//! * **Everything that touches the filesystem or starts a process is in
//!   here.** Outside this module the paths are rejected by the
//!   `gate_paths_only_inside_gates` audit (any reference: call, value,
//!   `use`, glob, rename, UFCS, const, macro body) *and* by rustc/clippy
//!   (`clippy::disallowed_methods`/`disallowed_types`, type-resolved, in
//!   `crates/*/clippy.toml`, denied at every crate root). The two layers
//!   cover each other: clippy sees through generics, deref and fn
//!   pointers; the audit sees code clippy never compiles (`cfg`'d-out
//!   blocks, uninvoked `macro_rules!`, orphan files) and whole crates
//!   (`libc::*`).
//! * **Destructive operations are plain functions.** [`destroy::trash_move`]
//!   and [`destroy::Envelope`] take an ordinary path: swamp reports, the
//!   human decides, and there is no plan/grant/confirmation token in
//!   front of the move any more. The only refusals left are OS-level
//!   (permission denied, path gone, cross-device).
//! * **Spawns name a [`spawn::Program`].** The enum is the allow-list;
//!   there is no `Command` outside this module and no way to run a
//!   program that is not a variant. Every run is counted.
//! * **Content reads are bounded** ([`read::bounded_read`]) except for
//!   swamp's own state files ([`read::read_owned_string`]), which only the
//!   store modules may name (the audit's per-group allow-list).
//!
//! Each submodule is one capability group; the audit's allow-list says
//! which modules may reference which group (default: none).

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod columns;
pub mod continuity;
pub mod destroy;
pub mod fs_space;
pub mod git;
#[cfg(target_os = "linux")]
pub mod inotify;
// Not target-gated: the fail-closed rules are pure over a fixture tree
// and are tested on both platforms (`occupancy::tests`'s `FakeProc`);
// only `occupancy::probe_paths`'s call into it is Linux-only.
pub mod procfs;
pub mod read;
pub mod spawn;
pub mod store;
pub mod sys;
pub mod systemd;

/// A swamp state directory (see [`store::StoreDir`]). Named here so any
/// module can hold or pass one; building one from a caller's path
/// (`StoreDir::at`) is the store modules' capability.
pub use store::StoreDir;

use std::io;
use std::path::{Path, PathBuf};

/// Metadata as the standard library returns it. Re-exported so callers
/// never name `std::fs`; every method on it reads fields of a `stat`
/// already taken, never the filesystem.
pub use std::fs::{DirEntry, FileType, Metadata, Permissions, ReadDir};
/// Accessors for `dev`/`ino`/`nlink`/`ctime`/`blocks`/`mode` on an
/// already-taken [`Metadata`]. Pure field reads.
#[cfg(unix)]
pub use std::os::unix::fs::{MetadataExt, PermissionsExt};

// ---------------------------------------------------------------------
// stat: one `lstat`/`stat` per call, no contents, no listing
// ---------------------------------------------------------------------

/// `lstat(2)`: never follows a symlink. What every reviewed member and
/// every adapter uses (`.oh/guardrails/symlinks-never-followed.md`).
pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    std::fs::symlink_metadata(path)
}

/// `stat(2)`, following symlinks. Only for the one question that is
/// about the link's *target*: which device a (canonicalized) root lives
/// on. Never for a reviewed member.
pub fn metadata_following(path: impl AsRef<Path>) -> io::Result<Metadata> {
    std::fs::metadata(path)
}

/// Whether something exists at `path`, following symlinks (the
/// semantics of `Path::exists`).
pub fn exists(path: impl AsRef<Path>) -> bool {
    path.as_ref().exists()
}

/// Whether `path` is a directory, following symlinks (the semantics of
/// `Path::is_dir`).
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    path.as_ref().is_dir()
}

/// Whether `path` is a regular file, following symlinks (the semantics
/// of `Path::is_file`).
pub fn is_file(path: impl AsRef<Path>) -> bool {
    path.as_ref().is_file()
}

/// Whether `path` itself is a regular file, **not** following a symlink.
pub fn is_real_file(path: impl AsRef<Path>) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::read_link(path)
}

/// The device a root lives on, following symlinks (a root given as a
/// link is measured where it points). `None` when it cannot be statted.
#[cfg(unix)]
pub fn device_of(path: impl AsRef<Path>) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
pub fn device_of(_path: impl AsRef<Path>) -> Option<u64> {
    None
}

// ---------------------------------------------------------------------
// list: one directory level per call
// ---------------------------------------------------------------------

/// Whether `path` can be opened for listing right now, without reading
/// a single entry: the "is this root still readable" probe a scope root
/// gets just before it is walked. `Err` carries the reason (missing,
/// permission denied).
pub fn probe_listable(path: impl AsRef<Path>) -> io::Result<()> {
    std::fs::read_dir(path).map(|_| ())
}

/// One directory level. The walker, the reviewed-unit snapshot and the
/// capped `shallow_list` are the only modules the audit allows to name
/// this; an adapter lists through `IdentifyCtx`, the report path never
/// lists at all (`.oh/guardrails/no-second-traversal-on-report-path.md`).
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    std::fs::read_dir(path)
}
