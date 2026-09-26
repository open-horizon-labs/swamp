//! #49 adversarial test: Docker Desktop's VM backing file is sparse --
//! its *apparent* (logical) size can be tens of gigabytes while its
//! *allocated* (actual on-disk) size is a small fraction of that. The
//! tempting shortcut this test catches is reporting `Docker.raw`'s
//! logical file length (`metadata().len()`) as its measured size, which
//! would wildly overstate real disk usage for every sparse disk image.

use std::fs;
use std::os::unix::fs::MetadataExt;

use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

#[test]
fn sparse_backing_file_reports_allocated_bytes_not_apparent_size() {
    let home = tempfile::tempdir().unwrap();
    let vm_data_dir = home
        .path()
        .join("Library/Containers/com.docker.docker/Data/vms/0/data");
    fs::create_dir_all(&vm_data_dir).unwrap();
    let backing_file = vm_data_dir.join("Docker.raw");

    // A sparse file: declare a large logical length via `set_len`
    // without writing any actual data, then write one small real block.
    // This is exactly how a real Docker.raw looks after Docker reclaims
    // space: a large virtual disk with only a fraction actually written.
    let f = fs::File::create(&backing_file).unwrap();
    f.set_len(200 * 1024 * 1024).unwrap(); // 200 MiB apparent
    drop(f);
    fs::write(vm_data_dir.join("small_real_file"), vec![1u8; 4_096]).unwrap();

    let apparent_len = fs::metadata(&backing_file).unwrap().len();
    assert_eq!(apparent_len, 200 * 1024 * 1024, "fixture sanity check");
    // The sparse file's *allocated* blocks are far smaller than its
    // apparent length -- this is the actual filesystem behavior being
    // relied on, not an assumption; assert it holds for this fixture
    // before trusting the rest of the test.
    let allocated_blocks_bytes = fs::metadata(&backing_file).unwrap().blocks() * 512;
    assert!(
        allocated_blocks_bytes < apparent_len / 2,
        "fixture filesystem did not actually create a sparse file (allocated {allocated_blocks_bytes} \
         vs apparent {apparent_len}); this test cannot validate sparse-awareness on this filesystem"
    );

    let mut env = std::collections::HashMap::new();
    // Never let this fixture reach the real Homebrew prefix.
    env.insert(
        "HOMEBREW_PREFIX".to_string(),
        home.path()
            .join("fixture-homebrew-prefix")
            .display()
            .to_string(),
    );
    let environment = Environment::fixture(home.path().to_path_buf(), env, Platform::MacOS);
    let registry = Registry::with_builtins();
    let disabled: Vec<String> = registry
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| id != "builtin-defaults" && id != "docker-desktop")
        .collect();
    let cfg = ScanConfig {
        defaults: false,
        disabled_detectors: disabled,
        enabled_detectors: Vec::new(),
        ..ScanConfig::default()
    };
    let scope = resolve_effective_scope(&environment, &cfg, &[], &registry, 1_000);

    let units = swamp_core::external::discover_and_measure(
        &scope,
        None,
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let unit = units
        .iter()
        .find(|u| u.detector_id == "docker-desktop" && u.path.ends_with("data"))
        .expect("the VM data directory is measured as its own external unit");

    // The measured unit must reflect the *allocated* reality (roughly
    // one file's worth of real blocks, plus the sparse file's own
    // allocated blocks -- nowhere near the 200 MiB apparent size).
    assert!(
        unit.bytes < 10 * 1024 * 1024,
        "measured {} bytes must be near allocated (~4KB + sparse file's real blocks), \
         not the 200 MiB apparent/logical size",
        unit.bytes
    );
    assert!(unit.bytes > 0);
}
