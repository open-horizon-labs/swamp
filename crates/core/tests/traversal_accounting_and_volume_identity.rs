//! #80: the walker's accounting, symlink handling and volume identity,
//! asserted against the filesystem rather than against itself, on
//! whichever OS the test runs on.
//!
//! The walker was already portable Unix code -- `st_blocks * 512`,
//! `(dev, ino)` hardlink dedup, `st_dev` boundary, `symlink_metadata`
//! everywhere. "Portable" was an inference from reading it, though, and
//! the properties it rests on are exactly the ones that go wrong
//! quietly: a sparse file counted by length instead of allocation
//! inflates a report by the size of a hole; a hardlinked member counted
//! twice invents bytes that cannot be reclaimed; a followed symlink
//! walks out of the tree it was asked about, and a symlink loop does it
//! forever.
//!
//! So the fixture here contains one of each, and the totals are checked
//! against `du`, which is the other program on the machine that answers
//! the same question. `du -skPx` reports allocated blocks and does not
//! follow symlinks, which is the same contract; `du -s --apparent-size`
//! reports logical length, which is deliberately *different* and is used
//! here to prove the sparse file really is sparse -- if the two agreed,
//! the fixture would not be testing anything.
//!
//! Where `du` is absent the test says so and skips that comparison
//! rather than passing quietly: an unavailable oracle is a gap in the
//! evidence, not a success.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use swamp_core::report::ArtifactKind;

/// Fills a fixture with every case that has ever made a size wrong.
/// Returns the root plus the allocated bytes the *filesystem* says the
/// files occupy, summed the way the walker is supposed to.
struct Fixture {
    root: PathBuf,
    /// Sum of `st_blocks * 512` over distinct inodes under `root`,
    /// computed here from `symlink_metadata` directly.
    expected_allocated: u64,
    /// Logical length of the sparse file.
    sparse_logical: u64,
    /// Allocated bytes of the sparse file.
    sparse_allocated: u64,
}

fn build(root: &Path) -> Fixture {
    fs::create_dir_all(root.join("nested/deeper")).unwrap();

    // Ordinary files, one per directory level.
    fs::write(root.join("a.bin"), vec![7u8; 40_960]).unwrap();
    fs::write(root.join("nested/b.bin"), vec![7u8; 20_480]).unwrap();
    fs::write(root.join("nested/deeper/c.bin"), vec![7u8; 8_192]).unwrap();

    // A hardlink pair: two names, one inode. Counted once.
    fs::hard_link(root.join("a.bin"), root.join("nested/a-alias.bin")).unwrap();

    // A sparse file: 64 MiB of address space, one byte written at the
    // end. Allocation is a couple of blocks; length is 64 MiB.
    let sparse = root.join("sparse.img");
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = fs::File::create(&sparse).unwrap();
        f.seek(SeekFrom::Start(64 * 1024 * 1024 - 1)).unwrap();
        f.write_all(&[1u8]).unwrap();
        f.sync_all().unwrap();
    }

    // A symlink loop, and a symlink pointing at a real directory. Either
    // one followed turns this fixture into an infinite walk or a double
    // count.
    std::os::unix::fs::symlink(root.join("loop-b"), root.join("loop-a")).unwrap();
    std::os::unix::fs::symlink(root.join("loop-a"), root.join("loop-b")).unwrap();
    std::os::unix::fs::symlink(root.join("nested"), root.join("nested-link")).unwrap();
    std::os::unix::fs::symlink(root.join("a.bin"), root.join("a-symlink.bin")).unwrap();

    let sparse_meta = fs::symlink_metadata(&sparse).unwrap();

    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut expected = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap().flatten() {
            let m = fs::symlink_metadata(e.path()).unwrap();
            if m.file_type().is_symlink() {
                continue;
            }
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() && seen.insert((m.dev(), m.ino())) {
                expected += m.blocks() * 512;
            }
        }
    }

    Fixture {
        root: root.to_path_buf(),
        expected_allocated: expected,
        sparse_logical: sparse_meta.len(),
        sparse_allocated: sparse_meta.blocks() * 512,
    }
}

fn du(args: &[&str], root: &Path) -> Option<u64> {
    let out = std::process::Command::new("du")
        .args(args)
        .arg(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let kib: u64 = text.split_whitespace().next()?.parse().ok()?;
    Some(kib * 1024)
}

/// The headline: what swamp reports for a unit equals what the
/// filesystem allocated to it, on a tree containing every shape that
/// makes the two diverge.
#[test]
fn allocated_bytes_match_the_filesystem_across_hardlinks_sparseness_and_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = build(&tmp.path().join("unit"));

    let row = swamp_core::walk::resize_artifact(&fx.root, ArtifactKind::Cache, 1_000);
    assert_eq!(
        row.bytes,
        fx.expected_allocated,
        "walker total {} != filesystem allocation {} on {}",
        row.bytes,
        fx.expected_allocated,
        fx.root.display()
    );
}

/// `du -skPx` answers the same question -- allocated blocks, no symlink
/// dereference, one filesystem -- so the two must agree. This is the
/// cross-check that does not share a line of code with the walker.
#[test]
fn the_total_agrees_with_du() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = build(&tmp.path().join("unit"));
    let row = swamp_core::walk::resize_artifact(&fx.root, ArtifactKind::Cache, 1_000);

    let Some(du_allocated) = du(&["-skPx"], &fx.root) else {
        eprintln!(
            "SKIPPED the du cross-check: `du -skPx` is unavailable or failed on this machine. \
             The walker's own accounting is still asserted by \
             allocated_bytes_match_the_filesystem_across_hardlinks_sparseness_and_symlinks; \
             what is missing here is the independent oracle."
        );
        return;
    };

    // `du` also counts the directories' own blocks, which swamp does not
    // attribute to a unit's files. On every filesystem this runs on that
    // is a handful of blocks; the assertion is that the two agree on the
    // *file* bytes, which dominate by three orders of magnitude here.
    let directories = 3u64;
    let slack = directories * 64 * 1024;
    assert!(
        du_allocated >= row.bytes && du_allocated - row.bytes <= slack,
        "du says {du_allocated}, swamp says {}, difference {} exceeds the {slack} bytes of \
         directory blocks du attributes and swamp does not",
        row.bytes,
        du_allocated.saturating_sub(row.bytes)
    );
}

/// Allocated is not logical. A 64 MiB sparse file occupies a few blocks,
/// and a report that said 64 MiB would be promising back space that was
/// never taken. `du --apparent-size` is the number swamp must *not*
/// produce.
#[test]
fn a_sparse_file_is_counted_by_allocation_not_by_length() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = build(&tmp.path().join("unit"));
    assert!(
        fx.sparse_allocated < fx.sparse_logical / 100,
        "the fixture's sparse file is not sparse on this filesystem \
         (allocated {}, logical {}); the rest of this test would prove nothing",
        fx.sparse_allocated,
        fx.sparse_logical
    );

    let row = swamp_core::walk::resize_artifact(&fx.root, ArtifactKind::Cache, 1_000);
    assert!(
        row.bytes < fx.sparse_logical,
        "the unit total {} includes the sparse file's full length {}",
        row.bytes,
        fx.sparse_logical
    );

    if let Some(apparent) = du(&["-s", "-k", "--apparent-size"], &fx.root) {
        assert!(
            apparent > row.bytes * 10,
            "apparent size {apparent} should dwarf the allocated total {}; if it does not, \
             this filesystem did not make the file sparse",
            row.bytes
        );
    } else {
        eprintln!(
            "SKIPPED the apparent-size comparison: `du --apparent-size` is unavailable \
             (BSD du has no such flag). The allocation assertions above still hold."
        );
    }
}

/// Two names, one inode, one count. Counting both is how a cleanup
/// report offers back bytes that removing one name does not free.
#[test]
fn a_hardlinked_file_is_counted_once_not_once_per_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hardlinks");
    fs::create_dir_all(root.join("b")).unwrap();
    fs::write(root.join("original.bin"), vec![3u8; 1_048_576]).unwrap();
    let one = fs::symlink_metadata(root.join("original.bin"))
        .unwrap()
        .blocks()
        * 512;
    for i in 0..8 {
        fs::hard_link(
            root.join("original.bin"),
            root.join(format!("b/link{i}.bin")),
        )
        .unwrap();
    }

    let row = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000);
    assert_eq!(
        row.bytes, one,
        "nine names for one inode must total one inode's allocation, not nine"
    );
}

/// A symlink loop is the case where "never follow symlinks" stops being
/// an accounting nicety and becomes the difference between finishing and
/// not. The assertion is simply that this returns.
#[test]
fn a_symlink_loop_terminates_and_contributes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("loops");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("real.bin"), vec![5u8; 4_096]).unwrap();
    let real = fs::symlink_metadata(root.join("real.bin"))
        .unwrap()
        .blocks()
        * 512;

    std::os::unix::fs::symlink(root.join("l2"), root.join("l1")).unwrap();
    std::os::unix::fs::symlink(root.join("l1"), root.join("l2")).unwrap();
    // A symlink to the root itself: following it once is an infinite
    // regress even without a two-link cycle.
    std::os::unix::fs::symlink(&root, root.join("self")).unwrap();

    let row = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000);
    assert_eq!(
        row.bytes, real,
        "symlinks must contribute nothing; a followed one would double or hang"
    );
}

/// A symlink to a directory inside the same unit is the quiet version of
/// the same bug: following it counts that directory's bytes twice, and
/// the report blames a unit for storage it does not hold.
#[test]
fn a_symlink_to_a_directory_does_not_double_count_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("aliased");
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/x.bin"), vec![9u8; 2_097_152]).unwrap();
    let once = fs::symlink_metadata(root.join("data/x.bin"))
        .unwrap()
        .blocks()
        * 512;
    std::os::unix::fs::symlink(root.join("data"), root.join("data-link")).unwrap();

    let row = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000);
    assert_eq!(row.bytes, once);
}

/// A vanished file is a fact about the moment, not an error: a build
/// directory being measured while a compiler writes to it is the normal
/// case, and a measurement that failed whenever a file disappeared
/// mid-walk would fail constantly.
#[test]
fn a_file_that_vanishes_mid_walk_does_not_fail_the_measurement() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("churn");
    fs::create_dir_all(&root).unwrap();
    for i in 0..64 {
        fs::write(root.join(format!("{i}.bin")), vec![1u8; 4_096]).unwrap();
    }
    let doomed = root.join("32.bin");

    let handle = std::thread::spawn({
        let doomed = doomed.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(1));
            let _ = fs::remove_file(&doomed);
        }
    });
    let row = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000);
    handle.join().unwrap();

    // Either 63 or 64 files, depending on the race. Both are correct;
    // what must not happen is a zero, a panic, or a hang.
    let one = 4_096u64;
    assert!(
        row.bytes >= 60 * one,
        "a concurrent unlink collapsed the measurement to {}",
        row.bytes
    );
}

// ---------------------------------------------------------------------
// Volume identity
// ---------------------------------------------------------------------

/// The growth store is keyed by `(device, canonical path)`. Both halves
/// matter and each covers a failure the other does not.
#[test]
fn volume_identity_is_stable_for_one_root_and_distinct_between_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();

    let id_a = swamp_core::growth::root_scoped_volume_id(&a);
    assert_eq!(
        id_a,
        swamp_core::growth::root_scoped_volume_id(&a),
        "the same root must key the same store every time"
    );
    assert_ne!(
        id_a,
        swamp_core::growth::root_scoped_volume_id(&b),
        "two roots on one device must not share a store"
    );

    // A symlinked spelling of the same root canonicalizes to it, so it
    // is the same store rather than a second, empty one that looks like
    // everything vanished.
    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(&a, &alias).unwrap();
    assert_eq!(id_a, swamp_core::growth::root_scoped_volume_id(&alias));
}

/// The documented limit, asserted so it cannot be forgotten: the id
/// includes `st_dev`, and `st_dev` is **not** stable across reboots for
/// every Linux mount. A device-minor change that comes from the kernel
/// enumerating devices in a different order will produce a different id
/// for the same directory, and swamp will start a fresh history rather
/// than continue the old one.
///
/// That is the safe direction. The alternative -- keying on the path
/// alone -- would join two genuinely different filesystems mounted at
/// the same path into one history, which is fabricated growth. A lost
/// baseline is visible ("no history yet"); a fabricated one is not.
///
/// The test states the dependency directly rather than simulating a
/// reboot: two directories that differ only in device must get different
/// ids, which is the same arithmetic the reboot case exercises.
#[test]
fn volume_identity_depends_on_the_device_and_a_changed_device_starts_a_new_history() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let canonical = fs::canonicalize(&root).unwrap();
    let device = fs::metadata(&canonical).unwrap().dev();

    // Reproduce the id's derivation with the real device and with a
    // hypothetical post-reboot one.
    let id_with = |dev: u64| {
        use std::os::unix::ffi::OsStrExt;
        let mut h = blake3::Hasher::new();
        h.update(&dev.to_le_bytes());
        h.update(canonical.as_os_str().as_bytes());
        u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap())
    };

    assert_eq!(
        id_with(device),
        swamp_core::growth::root_scoped_volume_id(&root),
        "this test's model of the id no longer matches the implementation"
    );
    assert_ne!(
        id_with(device),
        id_with(device ^ 1),
        "a different device number must key a different store: joining them would make one \
         filesystem's history speak for another's bytes"
    );
}

/// The block-size multiplier is the one thing a unit mistake hides
/// inside a plausible-looking number, and it differs by backend
/// (`f_bsize` on Darwin's `statfs`, `f_frsize` on Linux's
/// `statvfs`). `df -k` is the independent oracle: it asks the same
/// kernel through a different program, and it is what the old
/// implementation parsed, so agreeing with it is also the evidence
/// that replacing it changed no answer.
///
/// A 5% band, because a busy machine genuinely moves blocks between
/// the two calls. A factor-of-512 or factor-of-1024 mistake -- the
/// ones actually on offer here -- is nowhere near it.
#[test]
fn the_figure_agrees_with_df() {
    let tmp = tempfile::tempdir().unwrap();
    let ours =
        swamp_core::platform::fs_space::available_bytes(tmp.path()).expect("space for a temp dir");

    let Ok(out) = std::process::Command::new("df")
        .arg("-k")
        .arg(tmp.path())
        .output()
    else {
        eprintln!("SKIPPED: no `df` on this machine to cross-check against");
        return;
    };
    if !out.status.success() {
        eprintln!("SKIPPED: `df -k` failed on this machine");
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Not by field index: GNU df wraps a long device name onto a
    // second line, which is the bug this module exists to remove.
    // The available figure is the third number on the row, whichever
    // line it landed on.
    let numbers: Vec<u64> = text
        .lines()
        .skip(1)
        .flat_map(|l| l.split_whitespace())
        .filter_map(|f| f.parse::<u64>().ok())
        .collect();
    let Some(&avail_kib) = numbers.get(2) else {
        eprintln!("SKIPPED: could not read an available figure out of `df -k`:\n{text}");
        return;
    };
    let theirs = avail_kib * 1024;

    let (hi, lo) = (ours.max(theirs), ours.min(theirs).max(1));
    assert!(
        (hi - lo) * 20 <= hi,
        "statvfs/statfs says {ours} bytes available, `df -k` says {theirs}; \
         that is not measurement drift, it is a different unit"
    );
}
