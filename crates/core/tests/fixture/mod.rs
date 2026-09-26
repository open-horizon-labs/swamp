//! Reusable fixture builder for report golden tests.
//!
//! Builds, under a caller-supplied tmp root, a small multi-project tree
//! with real `git` checkouts (a main checkout plus a `git worktree add`
//! linked worktree), build-output/dependency directories, a nested second
//! repository, a shared cache directory outside any repo, loose files
//! outside any repo, and a mocked `docker system df -v` JSON fixture file.
//!
//! File contents are a deterministic byte pattern so sizes are exact and
//! reproducible across runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Paths and exact byte sizes for everything the fixture wrote. Not
/// every consumer (e.g. `dirs_and_files.rs`) needs every field -- each
/// integration test binary compiles this module separately, so an
/// unused field here is a per-binary lint, not a real dead-code signal.
#[allow(dead_code)]
pub struct Fixture {
    pub root: PathBuf,
    pub checkout: PathBuf,
    pub checkout_name: String,
    /// Only read by `report_golden.rs` (the image.source join case); other
    /// fixture consumers don't need it, so it's allowed to go unused there.
    #[allow(dead_code)]
    pub checkout_remote: String,
    pub linked_worktree: PathBuf,
    pub node_modules: PathBuf,
    pub node_modules_bytes: u64,
    pub target_dir: PathBuf,
    pub target_bytes: u64,
    pub dist_dir: PathBuf,
    pub dist_bytes: u64,
    pub nested_repo: PathBuf,
    pub nested_repo_build: PathBuf,
    pub nested_repo_build_bytes: u64,
    pub shared_cache: PathBuf,
    pub shared_cache_bytes: u64,
    pub loose_file: PathBuf,
    pub loose_file_bytes: u64,
    /// `loose_file`'s containing directory: since #R10 item 1, an
    /// unowned row is folded per directory (never per file), so a
    /// consumer asserting on the unowned row for a loose file must match
    /// against this, not `loose_file` itself.
    pub loose_dir: PathBuf,
    pub docker_facts: PathBuf,
    /// Only read by `report_golden.rs` (the compose-file-name join case);
    /// other fixture consumers don't need them.
    #[allow(dead_code)]
    pub compose_file: PathBuf,
    #[allow(dead_code)]
    pub compose_project_name: String,
}

/// Writes `size` bytes of a deterministic repeating pattern to `path`,
/// creating parent directories as needed.
fn write_pattern(path: &Path, size: u64) {
    fs::create_dir_all(path.parent().expect("path has parent")).expect("mkdir parent");
    let pattern: Vec<u8> = (0..=255u8).collect();
    let mut remaining = size as usize;
    let mut buf = Vec::with_capacity(size as usize);
    while remaining > 0 {
        let take = remaining.min(pattern.len());
        buf.extend_from_slice(&pattern[..take]);
        remaining -= take;
    }
    fs::write(path, &buf).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Writes `total_bytes` split evenly across `count` files under `dir`,
/// named `file-0`, `file-1`, ... Returns the exact total bytes written.
fn write_files(dir: &Path, count: u64, total_bytes: u64) -> u64 {
    let per_file = total_bytes / count;
    let remainder = total_bytes - per_file * count;
    let mut written = 0u64;
    for i in 0..count {
        let size = if i == count - 1 {
            per_file + remainder
        } else {
            per_file
        };
        write_pattern(&dir.join(format!("file-{i}")), size);
        written += size;
    }
    written
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Builds the full fixture tree under `tmp` and returns its paths and
/// exact byte sizes.
pub fn build(tmp: &Path) -> Fixture {
    // Production report paths are canonicalized at the public boundary.
    // Normalize the fixture once so every expected path (including linked
    // worktrees and nested repositories) uses the same namespace on macOS,
    // where temporary directories are commonly reachable as both /var and
    // /private/var.
    let tmp = fs::canonicalize(tmp).unwrap_or_else(|_| tmp.to_path_buf());
    fs::create_dir_all(&tmp).expect("mkdir tmp root");

    // --- main checkout ---
    let checkout = tmp.join("checkout");
    fs::create_dir_all(&checkout).expect("mkdir checkout");
    run_git(&checkout, &["init", "-q", "-b", "main"]);
    run_git(&checkout, &["config", "commit.gpgsign", "false"]);
    fs::write(checkout.join("README.md"), b"fixture checkout\n").expect("write README");
    run_git(&checkout, &["add", "README.md"]);
    run_git(&checkout, &["commit", "-q", "-m", "initial commit"]);
    let checkout_name = checkout
        .file_name()
        .and_then(|n| n.to_str())
        .expect("checkout has a name")
        .to_string();
    let checkout_remote = "https://example.com/fixture-org/checkout.git";
    run_git(&checkout, &["remote", "add", "origin", checkout_remote]);

    // node_modules/: ~3 MB across several files
    let node_modules = checkout.join("node_modules");
    let node_modules_bytes = write_files(&node_modules, 6, 3 * 1024 * 1024);

    // target/: ~2 MB
    let target_dir = checkout.join("target");
    let target_bytes = write_files(&target_dir, 4, 2 * 1024 * 1024);

    // dist/: ~1 MB. `dist` is an artifact only next to a marker of an
    // ecosystem that generates it; this checkout is a Node project.
    fs::write(checkout.join("package.json"), b"{\"name\": \"fixture\"}\n")
        .expect("write package.json");
    let dist_dir = checkout.join("dist");
    let dist_bytes = write_files(&dist_dir, 2, 1024 * 1024);

    // --- linked worktree ---
    let linked_worktree = tmp.join("checkout-linked");
    run_git(
        &checkout,
        &[
            "worktree",
            "add",
            "-q",
            linked_worktree
                .to_str()
                .expect("linked worktree path is utf8"),
            "-b",
            "linked-branch",
        ],
    );

    // --- compose file at the checkout root, naming a project that does
    // NOT match the checkout's own directory name (mirrors the real
    // `hiphi-staging` case #28 fixes) ---
    let compose_project_name = "fixture-stack".to_string();
    let compose_file = checkout.join("compose.yaml");
    fs::write(
        &compose_file,
        format!("name: {compose_project_name}\nservices:\n  web:\n    image: fixture/web\n"),
    )
    .expect("write compose fixture");

    // --- nested second repo, with its own build/ ---
    let nested_repo = checkout.join("nested-repo");
    fs::create_dir_all(&nested_repo).expect("mkdir nested repo");
    run_git(&nested_repo, &["init", "-q", "-b", "main"]);
    run_git(&nested_repo, &["config", "commit.gpgsign", "false"]);
    fs::write(nested_repo.join("README.md"), b"nested fixture repo\n")
        .expect("write nested README");
    run_git(&nested_repo, &["add", "README.md"]);
    run_git(&nested_repo, &["commit", "-q", "-m", "initial commit"]);
    // `build/` counts as an artifact next to a CMake marker.
    fs::write(nested_repo.join("CMakeLists.txt"), b"project(nested)\n")
        .expect("write CMakeLists.txt");
    let nested_repo_build = nested_repo.join("build");
    let nested_repo_build_bytes = write_files(&nested_repo_build, 2, 512 * 1024);

    // --- shared cache dir outside any repo ---
    // R3 sizes artifacts as allocated bytes (st_blocks*512), which rounds
    // up to the filesystem's block size (4096 on APFS). 1 MiB does not
    // split evenly into 3 files without a non-block-aligned remainder, so
    // this uses a total that is both divisible by 3 and by 4096, keeping
    // every file's logical size equal to its allocated size exactly.
    let shared_cache = tmp.join("cache").join(".cargo-registry");
    let shared_cache_bytes = write_files(&shared_cache, 3, 3 * 85 * 4096);

    // --- loose files outside any repo ---
    let loose_dir = tmp.join("loose");
    let loose_file = loose_dir.join("orphan.bin");
    let loose_file_bytes = 4096u64;
    write_pattern(&loose_file, loose_file_bytes);

    // --- mocked `docker system df -v` + `docker image inspect` JSON ---
    // This mirrors the real two-source shape: `docker system df -v
    // --format json` image rows carry NO `Labels` field at all (verified
    // live -- its image objects have exactly `[Containers, CreatedAt,
    // CreatedSince, Digest, ID, Repository, SharedSize, Size, Tag,
    // UniqueSize]`); labels only exist in `docker image inspect` output
    // under `Config.Labels`. "Images" below is the df shape (no Labels);
    // "ImageInspect" is the inspect shape, merged in by ID. Volume labels
    // *are* present directly in `docker system df -v` (also verified
    // live), so volumes keep their `Labels` field inline.
    //
    // Seven images exercise every R5 join rule and the base-image trap:
    //   aaaa - compose-project label matches the checkout -> joined, High.
    //   bbbb - unlabeled, named exactly like the project -> unowned;
    //          name similarity is never evidence.
    //   cccc - image.source matches the checkout's git remote -> joined, Medium.
    //   dddd - image.source points elsewhere, name unrelated -> unowned,
    //          with a base_image_source note, never attributed.
    //   eeee - THE BASE-IMAGE TRAP: named exactly like the project (like
    //          real-world `muness/unified-hifi-control` -> its own repo
    //          name) but its image.source points at the upstream base
    //          image it was built FROM (like real-world
    //          `linuxcontainers/alpine`), not the project itself -> must
    //          land unowned with a base_image_source note, never
    //          attributed to the project despite the name match.
    //   ffff - compose-project label matches NO discovered project (a
    //          real compose project name frequently differs from the
    //          repo name, e.g. `hiphi-staging` for `hiphi-relay`/
    //          `hiphi-authorizer`), but working_dir is a path inside the
    //          linked worktree -> joined, High, to the linked worktree,
    //          and the unmatched compose label never surfaces as a
    //          `compose_project` note because rule 2 already joined it.
    //   gggg - compose-project label matches no discovered project and
    //          carries no other evidence -> unowned, with a
    //          `compose_project=<name>` note so a later slice can still
    //          group it, even though this slice can't attribute it.
    //   a build-cache entry with no labels -> unowned (no evidence; no
    //          label source exists for build cache at all).
    //   a volume with a compose-project label matching the checkout ->
    //          joined, High (volumes get their labels straight from df -v).
    //   9999 (#28) - compose-project label matches no discovered
    //          project's own name, but the checkout root has a
    //          `compose.yaml` with `name: fixture-stack` -> joined, High,
    //          rule `compose_file_name`, to the checkout's main worktree.
    //          A second volume with the same label exercises the same
    //          rule for volumes.
    let docker_facts = tmp.join("docker_system_df_v.json");
    let elsewhere_source = "https://example.com/some-other-org/unrelated.git";
    let base_image_source = "https://github.com/example-org/some-upstream-base-image";
    let unmatched_compose_project = "fixture-compose-project-with-no-checkout";
    let docker_json = serde_json::json!({
        "Images": [
            {
                "ID": "sha256:aaaa000000000000000000000000000000000000000000000000000000aa",
                "Repository": "fixture/with-project",
                "Tag": "latest",
                "Size": "10485760",
                "SharedSize": "2097152",
                "UniqueSize": "8388608",
            },
            {
                "ID": "sha256:bbbb000000000000000000000000000000000000000000000000000000bb",
                "Repository": checkout_name.clone(),
                "Tag": "latest",
                "Size": "5242880",
                "SharedSize": "1048576",
                "UniqueSize": "4194304",
            },
            {
                "ID": "sha256:cccc000000000000000000000000000000000000000000000000000000cc",
                "Repository": "fixture/source-match",
                "Tag": "latest",
                "Size": "3145728",
                "SharedSize": "1048576",
                "UniqueSize": "2097152",
            },
            {
                "ID": "sha256:dddd000000000000000000000000000000000000000000000000000000dd",
                "Repository": "fixture/source-elsewhere",
                "Tag": "latest",
                "Size": "2097152",
                "SharedSize": "524288",
                "UniqueSize": "1572864",
            },
            {
                "ID": "sha256:eeee000000000000000000000000000000000000000000000000000000ee",
                "Repository": checkout_name.clone(),
                "Tag": "base-image-trap",
                "Size": "1572864",
                "SharedSize": "262144",
                "UniqueSize": "1310720",
            },
            {
                "ID": "sha256:ffff000000000000000000000000000000000000000000000000000000ff",
                "Repository": "fixture/working-dir-join",
                "Tag": "latest",
                "Size": "1048576",
                "SharedSize": "131072",
                "UniqueSize": "917504",
            },
            {
                "ID": "sha256:aaaa111111111111111111111111111111111111111111111111111111aa",
                "Repository": "fixture/unmatched-compose-project",
                "Tag": "latest",
                "Size": "786432",
                "SharedSize": "65536",
                "UniqueSize": "720896",
            },
            {
                "ID": "sha256:9999000000000000000000000000000000000000000000000000000099",
                "Repository": "fixture/compose-file-name-join",
                "Tag": "latest",
                "Size": "655360",
                "SharedSize": "65536",
                "UniqueSize": "589824",
            }
        ],
        "ImageInspect": [
            {
                "Id": "sha256:aaaa000000000000000000000000000000000000000000000000000000aa",
                "RepoTags": ["fixture/with-project:latest"],
                "Config": { "Labels": { "com.docker.compose.project": checkout_name.clone() } },
            },
            {
                "Id": "sha256:cccc000000000000000000000000000000000000000000000000000000cc",
                "RepoTags": ["fixture/source-match:latest"],
                "Config": { "Labels": { "org.opencontainers.image.source": checkout_remote } },
            },
            {
                "Id": "sha256:dddd000000000000000000000000000000000000000000000000000000dd",
                "RepoTags": ["fixture/source-elsewhere:latest"],
                "Config": { "Labels": { "org.opencontainers.image.source": elsewhere_source } },
            },
            {
                "Id": "sha256:eeee000000000000000000000000000000000000000000000000000000ee",
                "RepoTags": [format!("{checkout_name}:base-image-trap")],
                "Config": { "Labels": { "org.opencontainers.image.source": base_image_source } },
            },
            {
                "Id": "sha256:ffff000000000000000000000000000000000000000000000000000000ff",
                "RepoTags": ["fixture/working-dir-join:latest"],
                "Config": { "Labels": {
                    "com.docker.compose.project": unmatched_compose_project,
                    "com.docker.compose.project.working_dir": linked_worktree.to_string_lossy(),
                } },
            },
            {
                "Id": "sha256:aaaa111111111111111111111111111111111111111111111111111111aa",
                "RepoTags": ["fixture/unmatched-compose-project:latest"],
                "Config": { "Labels": { "com.docker.compose.project": unmatched_compose_project } },
            },
            {
                "Id": "sha256:9999000000000000000000000000000000000000000000000000000099",
                "RepoTags": ["fixture/compose-file-name-join:latest"],
                "Config": { "Labels": { "com.docker.compose.project": compose_project_name.clone() } },
            }
        ],
        "BuildCache": [
            {
                "ID": "buildcache-no-evidence-0001",
                "Size": "1048576",
            }
        ],
        "Volumes": [
            {
                "Name": "fixture-project-volume",
                "Labels": format!("com.docker.compose.project={checkout_name}"),
                "Size": "262144",
            },
            {
                "Name": "fixture-stack-compose-name-volume",
                "Labels": format!("com.docker.compose.project={compose_project_name}"),
                "Size": "131072",
            }
        ]
    });
    fs::write(
        &docker_facts,
        serde_json::to_vec_pretty(&docker_json).expect("serialize docker fixture"),
    )
    .expect("write docker fixture");

    Fixture {
        root: tmp.to_path_buf(),
        checkout,
        checkout_name,
        checkout_remote: checkout_remote.to_string(),
        linked_worktree,
        node_modules,
        node_modules_bytes,
        target_dir,
        target_bytes,
        dist_dir,
        dist_bytes,
        nested_repo,
        nested_repo_build,
        nested_repo_build_bytes,
        shared_cache,
        shared_cache_bytes,
        loose_dir,
        loose_file,
        loose_file_bytes,
        docker_facts,
        compose_file,
        compose_project_name,
    }
}
