//! The Trash mover: swamp reports, the human decides. This module does
//! exactly one authorized-by-the-human-keyboard thing -- move a path (or
//! a bounded, named set of paths) into the platform Trash -- and nothing
//! resembling an automated recheck-then-veto gate. There is no stored
//! "grant", no plan-approval token and no drift refusal: a human marked
//! this in the TUI (or ran a shell command directly) and pressed Enter,
//! having just been shown the current facts. The only way any of these
//! functions refuse is an ordinary OS-level failure -- permission
//! denied, the path is gone, or a cross-device rename with no
//! permanent-delete fallback.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn plain_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        bail!("refused: `{name}` is not a plain file name inside the Trash");
    }
    Ok(())
}

/// What a Trash move did: the anchor it moved and where it went. The one
/// input `git_worktree_prune` takes besides the linked worktree's common
/// dir, so a prune only ever follows a move of that worktree.
#[must_use = "a Trash move's receipt says where the unit went"]
#[derive(Debug)]
pub struct Trashed {
    anchor: PathBuf,
    dest: PathBuf,
}

impl Trashed {
    /// What was moved, at its original path.
    pub fn anchor(&self) -> &Path {
        &self.anchor
    }

    /// Where the unit went.
    pub fn path(&self) -> &Path {
        &self.dest
    }

    pub fn into_path(self) -> PathBuf {
        self.dest
    }
}

/// `trash_root/dest_name` on macOS (a plain rename target: `~/.Trash`);
/// on Linux, the freedesktop Trash spec's `files/` subdirectory, so
/// `write_trashinfo_sidecar` can find `info/` beside it.
fn items_dir(trash_root: &Path) -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        trash_root.join("files")
    }
    #[cfg(not(target_os = "linux"))]
    {
        trash_root.to_path_buf()
    }
}

/// Linux only: the freedesktop Trash spec's per-item metadata
/// (`$trash/info/<name>.trashinfo`, next to `$trash/files/<name>`
/// [`items_dir`] just moved into). Best-effort -- a trash manager that
/// cannot find this sidecar still sees the file under `files/`, so a
/// failure here does not undo an already-completed move.
#[cfg(target_os = "linux")]
fn write_trashinfo_sidecar(trash_root: &Path, dest_name: &str, original: &Path) -> Result<()> {
    let info_dir = trash_root.join("info");
    std::fs::create_dir_all(&info_dir)?;
    let info_path = info_dir.join(format!("{dest_name}.trashinfo"));
    if std::fs::symlink_metadata(&info_path).is_ok() {
        // A previous attempt in the same second already wrote one for
        // this exact dest_name; leave it rather than clobbering it.
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let content = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        trashinfo_percent_encode(&original.to_string_lossy()),
        trashinfo_iso8601(now)
    );
    std::fs::write(&info_path, content)?;
    Ok(())
}

/// Percent-encodes the bytes the freedesktop Trash spec reserves in a
/// `Path=` value (`%`, control bytes, and the ones that would break a
/// desktop-entry-style key file: `\n`, `\r`, `\t`). Anything ASCII
/// printable and not one of those passes through, which keeps ordinary
/// Unix paths readable while still round-tripping exactly.
#[cfg(target_os = "linux")]
fn trashinfo_percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'%' | b'\n' | b'\r' | b'\t' | 0..=0x1f | 0x7f => {
                out.push_str(&format!("%{b:02X}"));
            }
            _ => out.push(b as char),
        }
    }
    out
}

/// A UTC timestamp in the spec's `YYYY-MM-DDThh:mm:ss` form (no
/// timezone offset field -- callers, including the reference
/// implementation, read a bare local time). Computed from the Unix
/// epoch with plain civil-calendar arithmetic (Howard Hinnant's
/// `days_from_civil`, doubly reviewed and public domain) rather than a
/// new dependency for one timestamp.
#[cfg(target_os = "linux")]
fn trashinfo_iso8601(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = (unix_secs % 86_400) as i64;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}")
}

/// Moves `anchor` to the Trash (one `rename`, same volume):
/// `trash_root/dest_name` on macOS; on Linux, `trash_root/files/dest_name`
/// with a `trash_root/info/dest_name.trashinfo` sidecar (the freedesktop
/// Trash spec), so a desktop file manager's own Trash view finds it.
/// Returns where the moved item went. The only refusals are OS-level:
/// the path is gone, permission denied, or the Trash is on a different
/// device with no permanent-delete fallback.
pub fn trash_move(anchor: &Path, trash_root: &Path, dest_name: &str) -> Result<Trashed> {
    plain_name(dest_name)?;
    // Checked before anything is created under `trash_root`: a
    // cross-device `trash_root` has no permanent-delete fallback here,
    // and creating the freedesktop `files/` subdirectory only to have
    // the rename itself fail with EXDEV would leave an empty directory
    // behind as if something had been copied there.
    if let Ok(anchor_meta) = std::fs::symlink_metadata(anchor)
        && let Ok(trash_meta) = std::fs::metadata(trash_root)
    {
        use std::os::unix::fs::MetadataExt;
        if trash_meta.dev() != anchor_meta.dev() {
            bail!(
                "refused: {} is on a different filesystem than {}; cross-device link (no \
                 permanent-delete fallback)",
                trash_root.display(),
                anchor.display()
            );
        }
    }
    let items = items_dir(trash_root);
    std::fs::create_dir_all(&items).context("could not create the Trash directory")?;
    let dest = items.join(dest_name);
    if std::fs::symlink_metadata(&dest).is_ok() {
        bail!("refused: {} already exists in the Trash", dest.display());
    }
    std::fs::rename(anchor, &dest)
        .with_context(|| format!("rename to Trash failed for {}", anchor.display()))?;
    #[cfg(target_os = "linux")]
    {
        // Best-effort: the move already happened; a sidecar that could
        // not be written does not undo it.
        let _ = write_trashinfo_sidecar(trash_root, dest_name, anchor);
    }
    Ok(Trashed {
        anchor: anchor.to_path_buf(),
        dest,
    })
}

/// A recovery envelope: one directory inside the Trash that receives a
/// multi-member unit (a session and its sidecars, a Cargo group) member
/// by member, next to its own `restore.json`.
#[derive(Debug)]
pub struct Envelope {
    dir: PathBuf,
    anchor: PathBuf,
    moved: Vec<(PathBuf, PathBuf)>,
}

impl Envelope {
    /// Creates `trash_root/name` (or reuses it if a previous attempt in
    /// the same second created it). When `same_device_as` is given,
    /// refuses a Trash on another volume: a cross-device move would be a
    /// copy plus a delete, and there is no permanent-delete fallback.
    pub fn open(
        anchor: &Path,
        trash_root: &Path,
        name: &str,
        same_device_as: Option<u64>,
    ) -> Result<Envelope> {
        plain_name(name)?;
        std::fs::create_dir_all(trash_root).context("could not create the Trash directory")?;
        if let Some(dev) = same_device_as {
            use std::os::unix::fs::MetadataExt;
            if std::fs::metadata(trash_root)?.dev() != dev {
                bail!("cross-device Trash unsupported; no permanent fallback");
            }
        }
        let items = items_dir(trash_root);
        std::fs::create_dir_all(&items).context("could not create the Trash directory")?;
        let dir = items.join(name);
        std::fs::create_dir_all(&dir).context("could not create Trash envelope")?;
        #[cfg(target_os = "linux")]
        {
            // The envelope directory itself is the trashed item, from
            // the multi-member unit's own anchor (the session path, the
            // Cargo group's selected member) -- one sidecar for the
            // envelope, not one per moved member.
            let _ = write_trashinfo_sidecar(trash_root, name, anchor);
        }
        Ok(Envelope {
            dir,
            anchor: anchor.to_path_buf(),
            moved: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn anchor(&self) -> &Path {
        &self.anchor
    }

    /// Writes (or rewrites) the envelope's own `restore.json`, synced.
    pub fn write_manifest<T: serde::Serialize + ?Sized>(&self, manifest: &T) -> Result<()> {
        let p = self.dir.join("restore.json");
        let bytes = serde_json::to_vec_pretty(manifest)?;
        std::fs::write(&p, bytes).context("writing restore.json")?;
        std::fs::File::open(&p)?.sync_all()?;
        Ok(())
    }

    /// Moves one member into the envelope as `dest_name`. The only
    /// refusal is an OS-level rename failure (the member is gone, or a
    /// name collision inside the envelope).
    pub fn move_member(&mut self, member: &Path, dest_name: &str) -> Result<PathBuf> {
        plain_name(dest_name)?;
        let to = self.dir.join(dest_name);
        std::fs::rename(member, &to)
            .with_context(|| format!("rename to Trash failed for {}", member.display()))?;
        self.moved.push((member.to_path_buf(), to.clone()));
        Ok(to)
    }

    /// Moves every member moved so far back where it came from, newest
    /// first. Returns what could not be restored.
    pub fn roll_back(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while let Some((from, to)) = self.moved.pop() {
            if std::fs::symlink_metadata(&from).is_ok() {
                failures.push(format!("{} reappeared", from.display()));
            } else if let Err(e) = std::fs::rename(&to, &from) {
                failures.push(e.to_string());
            }
        }
        failures
    }
}

/// Copies one compiled output out of `from` into `dest_dir` (or
/// `dest_dir/sub`) before the unit it belongs to is trashed
/// (`--keep-executables`).
pub fn copy_preserved(from: &Path, dest_dir: &Path, sub: Option<&str>) -> Result<PathBuf> {
    let mut dest_dir = dest_dir.to_path_buf();
    if let Some(sub) = sub {
        plain_name(sub)?;
        dest_dir = dest_dir.join(sub);
    }
    std::fs::create_dir_all(&dest_dir)?;
    let name = from.file_name().context("file has a name")?;
    let to = dest_dir.join(name);
    std::fs::copy(from, &to)
        .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
    Ok(to)
}

/// `docker image rm <id>` / `docker volume rm <name>`: permanent, in the
/// daemon. Returns the daemon's own refusal text when it declines.
pub fn docker_remove(removal: &crate::docker::Removal) -> std::result::Result<(), String> {
    let (kind, id) = match removal {
        crate::docker::Removal::Image { id } => ("image", id.clone()),
        crate::docker::Removal::Volume { name } => ("volume", name.clone()),
        crate::docker::Removal::Refused(why) => return Err((*why).to_string()),
    };
    if !crate::fs_gate::spawn::is_docker_ref(&id) {
        return Err(format!("refused: `{id}` is not a Docker object reference"));
    }
    let args: Vec<std::ffi::OsString> = vec![kind.into(), "rm".into(), id.into()];
    let out = super::spawn::run_unchecked(
        super::spawn::Program::Docker,
        &args,
        Duration::from_secs(60),
    )
    .map_err(|e| format!("docker: {e}"))?;
    if out.success() {
        return Ok(());
    }
    let err = out.stderr_lossy().trim().to_string();
    Err(if err.is_empty() {
        "docker refused the removal without saying why".to_string()
    } else {
        err
    })
}

/// `git -C <repo> worktree prune`, after the linked worktree at `common`'s
/// owning checkout went to the Trash. `common` is the `.git` common dir
/// the caller read from the worktree before moving it. Best-effort; the
/// move already happened.
pub fn git_worktree_prune(common: &Path) -> Result<()> {
    let repo = common.parent().unwrap_or(common);
    let args: Vec<std::ffi::OsString> = vec![
        "-C".into(),
        repo.as_os_str().to_owned(),
        "worktree".into(),
        "prune".into(),
    ];
    super::spawn::run_unchecked(super::spawn::Program::Git, &args, Duration::from_secs(60))?;
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod linux_trashinfo_tests {
    use super::*;

    #[test]
    fn percent_encodes_only_the_reserved_bytes() {
        assert_eq!(
            trashinfo_percent_encode("/home/me/build dir"),
            "/home/me/build dir"
        );
        assert_eq!(trashinfo_percent_encode("100%"), "100%25");
        assert_eq!(trashinfo_percent_encode("a\nb\tc\r"), "a%0Ab%09c%0D");
    }

    #[test]
    fn iso8601_matches_the_spec_shape() {
        // 2024-01-02T03:04:05Z
        let secs = 1_704_164_645u64;
        assert_eq!(trashinfo_iso8601(secs), "2024-01-02T03:04:05");
    }

    #[test]
    fn items_dir_nests_under_files() {
        let root = Path::new("/tmp/Trash");
        assert_eq!(items_dir(root), root.join("files"));
    }

    /// A multi-member unit (a Cargo group, an agent session) goes into
    /// one envelope, member by member; every member the caller names
    /// moves, and the envelope itself -- not each member -- gets the one
    /// `.trashinfo` sidecar.
    #[test]
    fn a_multi_member_envelope_moves_every_member_and_gets_one_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let session = root.join("session");
        std::fs::create_dir_all(&session).unwrap();
        let members = [session.join("transcript.jsonl"), session.join("meta.json")];
        for m in &members {
            std::fs::write(m, b"fixture content").unwrap();
        }
        let trash_root = root.join("Trash");

        use std::os::unix::fs::MetadataExt;
        let dev = std::fs::metadata(&session).unwrap().dev();
        let mut envelope = Envelope::open(&session, &trash_root, "session-1", Some(dev)).unwrap();
        for (i, m) in members.iter().enumerate() {
            envelope.move_member(m, &i.to_string()).unwrap();
        }
        for m in &members {
            assert!(!m.exists());
        }
        assert!(envelope.path().join("0").exists());
        assert!(envelope.path().join("1").exists());
        assert_eq!(envelope.path(), trash_root.join("files/session-1"));
        assert!(
            trash_root.join("info/session-1.trashinfo").exists(),
            "the envelope itself is the trashed item and gets one sidecar"
        );
    }
}
