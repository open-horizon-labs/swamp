//! #80: swamp's bounded parallel walker measured against a generic
//! library walker, for correctness first and speed second.
//!
//! The reuse question the issue asks is "could `walkdir` (or `jwalk`, or
//! clean-dev-dirs' scanner) do this instead". The honest way to answer it
//! is not to read their READMEs but to run one over the same tree and see
//! what comes out, so the first test here does exactly that: a plain
//! `walkdir` sum over a fixture with a hardlink pair, a sparse file and a
//! symlink is compared to swamp's figure, and to the filesystem.
//!
//! The point is not that `walkdir` is wrong -- it walks correctly and its
//! symlink default is the safe one. The point is that a *walker* is not a
//! *measurement*: allocated-vs-logical bytes, hardlink dedup by
//! `(dev, ino)` and a same-filesystem boundary are swamp's semantics, and
//! anything reused would have to be wrapped in all three anyway. The
//! failing assertion below is what that costs if the wrapping is skipped.
//!
//! The benchmark is `#[ignore]`d and prints rather than asserts: wall
//! time on a shared CI runner is not a property to gate a merge on. CI
//! runs it explicitly on both targets so the numbers in
//! `docs/platform.md` come from the machines they describe.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use swamp_core::report::ArtifactKind;

/// A tree with one of each accounting hazard.
fn hazard_fixture(root: &Path) {
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("plain.bin"), vec![1u8; 65_536]).unwrap();
    fs::hard_link(root.join("plain.bin"), root.join("nested/alias.bin")).unwrap();
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = fs::File::create(root.join("sparse.img")).unwrap();
        f.seek(SeekFrom::Start(32 * 1024 * 1024 - 1)).unwrap();
        f.write_all(&[1u8]).unwrap();
        f.sync_all().unwrap();
    }
    std::os::unix::fs::symlink(root.join("nested"), root.join("nested-link")).unwrap();
}

/// What a generic walker gives you if you sum `metadata.len()` -- the
/// shape clean-dev-dirs' size pass uses, and the obvious thing to write.
fn walkdir_logical_sum(root: &Path) -> u64 {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// The same walk with swamp's three rules applied on top.
fn walkdir_with_swamp_semantics(root: &Path) -> u64 {
    let root_dev = fs::symlink_metadata(root).unwrap().dev();
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_map(Result::ok)
    {
        let Ok(m) = entry.metadata() else { continue };
        if !m.is_file() || m.dev() != root_dev {
            continue;
        }
        if m.nlink() > 1 && !seen.insert((m.dev(), m.ino())) {
            continue;
        }
        total += m.blocks() * 512;
    }
    total
}

/// The evidence behind the reuse decision: a generic walker used the
/// obvious way reports a number that is wrong in both directions at once
/// -- far too large, because it counts the sparse file's holes and the
/// hardlink twice; and it would be too small on a tree that crossed a
/// mount. Reused code would have to carry swamp's semantics regardless,
/// which is what the second sum shows.
#[test]
fn a_generic_walker_needs_swamps_semantics_to_reach_swamps_number() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("unit");
    hazard_fixture(&root);

    let swamp = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000).bytes;
    let naive = walkdir_logical_sum(&root);
    let wrapped = walkdir_with_swamp_semantics(&root);

    assert!(
        naive > swamp * 10,
        "the naive logical sum ({naive}) should dwarf the allocated total ({swamp}); if it does \
         not, this filesystem gave the fixture no sparse file and the comparison proves nothing"
    );
    assert_eq!(
        wrapped, swamp,
        "walkdir with allocated bytes, (dev, ino) dedup and a same-filesystem boundary must \
         reach the same figure as swamp's walker -- if it does not, one of the two is wrong"
    );
}

/// `walkdir`'s symlink default is `follow_links(false)`, the safe one.
/// Asserted because the reuse assessment leans on it: a library whose
/// default followed links would have to be configured correctly at every
/// call site, and one missed call site walks out of the tree.
#[test]
fn the_library_walkers_default_is_not_to_follow_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("linked");
    fs::create_dir_all(root.join("real")).unwrap();
    fs::write(root.join("real/x.bin"), vec![2u8; 4_096]).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();

    let files: Vec<PathBuf> = walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    assert_eq!(
        files.len(),
        1,
        "a default walkdir walk followed the symlink and found {files:?}"
    );
}

// ---------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------

fn generated_tree(root: &Path, dirs: usize, files_per_dir: usize) -> u64 {
    let mut bytes = 0u64;
    for d in 0..dirs {
        let dir = root.join(format!("d{:03}/sub{}", d / 16, d % 16));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..files_per_dir {
            let path = dir.join(format!("f{f}.o"));
            fs::write(&path, vec![0u8; 1024 + (f % 7) * 512]).unwrap();
            bytes += fs::symlink_metadata(&path).unwrap().blocks() * 512;
        }
    }
    bytes
}

/// Prints, does not assert. Run with:
///
/// ```text
/// cargo test -p swamp-core --test walk_library_comparison -- --ignored --nocapture
/// ```
///
/// Two shapes, because they stress different things: many small files in
/// few directories (where a parallel walker has little to parallelise)
/// and many directories (where it has a lot).
#[test]
#[ignore = "benchmark: prints timings, asserts only correctness equivalence"]
fn benchmark_swamp_walker_against_walkdir() {
    for (dirs, files_per_dir) in [(16usize, 2_000usize), (512, 40)] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("bench");
        let expected = generated_tree(&root, dirs, files_per_dir);

        // Warm the directory cache for both, so the first one measured
        // does not pay for the second.
        let _ = walkdir_with_swamp_semantics(&root);
        let _ = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000);

        let t0 = std::time::Instant::now();
        let swamp = swamp_core::walk::resize_artifact(&root, ArtifactKind::Cache, 1_000).bytes;
        let swamp_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = std::time::Instant::now();
        let wrapped = walkdir_with_swamp_semantics(&root);
        let walkdir_ms = t1.elapsed().as_secs_f64() * 1000.0;

        println!(
            "WALK BENCH os={} dirs={dirs} files_per_dir={files_per_dir} files={} \
             swamp={swamp_ms:.1}ms walkdir={walkdir_ms:.1}ms ratio={:.2}x",
            std::env::consts::OS,
            dirs * files_per_dir,
            walkdir_ms / swamp_ms.max(f64::MIN_POSITIVE)
        );

        // The benchmark still asserts the one thing a benchmark must:
        // that both walks measured the same tree.
        assert_eq!(swamp, expected, "swamp's walk missed bytes");
        assert_eq!(wrapped, expected, "the walkdir reference missed bytes");
    }
}
