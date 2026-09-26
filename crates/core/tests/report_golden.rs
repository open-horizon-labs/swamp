//! Golden test for the R1 report contract.
//!
//! This test is expected to be RED until R2 (project discovery) lands:
//! `report()` is still the R1 stub that returns zero projects. The first
//! assertion below is the gate named in issue #20 and the epic (#19): a
//! report that discovers zero projects is a failure, not a passing empty
//! case.

#[path = "fixture/mod.rs"]
mod fixture;

use swamp_core::report::report;

#[test]
fn report_matches_fixture_and_reconciles() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let fx = fixture::build(tmp.path());

    let r = report(&fx.root, Some(&fx.docker_facts)).expect("report() should not error");

    // (a) the product gate: a report over a root with real checkouts must
    // discover at least one project. `report()` is currently the R1 stub
    // and always returns zero, so this is expected to fail here.
    assert!(!r.projects.is_empty(), "zero projects discovered");

    // (b) exact expected rows, once discovery/attribution exist.
    let checkout_project = r
        .projects
        .iter()
        .find(|p| p.name == fx.checkout_name)
        .unwrap_or_else(|| panic!("no project row for checkout {}", fx.checkout_name));

    let main_worktree = checkout_project
        .worktrees
        .iter()
        .find(|w| w.path == fx.checkout)
        .expect("main worktree row for checkout");
    assert_eq!(main_worktree.kind, swamp_core::report::WorktreeKind::Main);

    let linked_worktree = checkout_project
        .worktrees
        .iter()
        .find(|w| w.path == fx.linked_worktree)
        .expect("linked worktree row for checkout-linked");
    assert_eq!(
        linked_worktree.kind,
        swamp_core::report::WorktreeKind::Linked
    );

    let node_modules_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path == fx.node_modules)
        .expect("node_modules artifact row");
    assert_eq!(node_modules_row.bytes, fx.node_modules_bytes);
    assert_eq!(
        node_modules_row.kind,
        swamp_core::report::ArtifactKind::DependencyTree
    );

    let target_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path == fx.target_dir)
        .expect("target artifact row");
    assert_eq!(target_row.bytes, fx.target_bytes);
    assert_eq!(
        target_row.kind,
        swamp_core::report::ArtifactKind::BuildOutput
    );

    let dist_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path == fx.dist_dir)
        .expect("dist artifact row");
    assert_eq!(dist_row.bytes, fx.dist_bytes);
    assert_eq!(dist_row.kind, swamp_core::report::ArtifactKind::BuildOutput);

    let nested_project = r
        .projects
        .iter()
        .find(|p| p.worktrees.iter().any(|w| w.path == fx.nested_repo))
        .expect("nested repo project row");
    let nested_worktree = nested_project
        .worktrees
        .iter()
        .find(|w| w.path == fx.nested_repo)
        .expect("nested repo worktree row");
    let nested_build_row = nested_worktree
        .artifacts
        .iter()
        .find(|a| a.path == fx.nested_repo_build)
        .expect("nested repo build artifact row");
    assert_eq!(nested_build_row.bytes, fx.nested_repo_build_bytes);

    // shared cache and loose files are outside any checkout: unowned.
    let shared_cache_row = r
        .unowned
        .iter()
        .find(|u| u.path_or_object == fx.shared_cache.display().to_string())
        .expect("shared cache unowned row");
    assert_eq!(shared_cache_row.bytes, fx.shared_cache_bytes);

    // #R10 item 1: unowned rows fold per directory, never per file, so
    // the loose file's row is keyed by its containing directory.
    let loose_row = r
        .unowned
        .iter()
        .find(|u| u.path_or_object == fx.loose_dir.display().to_string())
        .expect("loose file's directory unowned row");
    assert_eq!(loose_row.bytes, fx.loose_file_bytes);

    // aaaa: compose-project label matches the checkout name -> joined,
    // High confidence, under the checkout's main worktree.
    let aaaa_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path.to_string_lossy().contains("fixture/with-project"))
        .expect("compose-label-joined docker image row");
    assert_eq!(aaaa_row.kind, swamp_core::report::ArtifactKind::DockerImage);
    assert_eq!(aaaa_row.confidence, swamp_core::entities::Confidence::High);
    assert_eq!(aaaa_row.bytes, 8_388_608);

    // bbbb: unlabeled, named exactly like the project -> unowned. Name
    // similarity is never evidence.
    let bbbb_row = r
        .unowned
        .iter()
        .find(|u| u.path_or_object == format!("{}:latest", fx.checkout_name))
        .expect("unlabeled same-name-as-project docker image unowned row");
    assert_eq!(
        bbbb_row.reason,
        swamp_core::report::UnownedReason::DockerNoJoin
    );
    assert!(
        !main_worktree.artifacts.iter().any(|a| {
            a.kind == swamp_core::report::ArtifactKind::DockerImage
                && a.path.to_string_lossy().contains(&fx.checkout_name)
                && !a.path.to_string_lossy().contains("fixture/")
        }),
        "name similarity alone must never attribute a docker object to a project"
    );

    // cccc: org.opencontainers.image.source matches the checkout's git
    // remote -> joined, Medium confidence.
    let cccc_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path.to_string_lossy().contains("fixture/source-match"))
        .expect("image.source-joined docker image row");
    assert_eq!(
        cccc_row.confidence,
        swamp_core::entities::Confidence::Medium
    );
    assert!(
        !fx.checkout_remote.is_empty(),
        "fixture must configure a checkout remote for the image.source join to match"
    );

    // dddd: org.opencontainers.image.source points elsewhere -> unowned,
    // with a base_image_source note, never attributed to any project.
    let dddd_row = r
        .unowned
        .iter()
        .find(|u| u.path_or_object.contains("fixture/source-elsewhere"))
        .expect("elsewhere-sourced docker image unowned row");
    assert_eq!(
        dddd_row.reason,
        swamp_core::report::UnownedReason::DockerNoJoin
    );
    assert!(
        dddd_row
            .note
            .as_deref()
            .is_some_and(|n| n.contains("base_image_source")),
        "elsewhere-sourced image must carry a base_image_source note: {dddd_row:?}"
    );

    // eeee: THE BASE-IMAGE TRAP. Named exactly like the project (mirrors
    // real-world `muness/unified-hifi-control`) but its image.source
    // points at an upstream base image it was built FROM (mirrors real-
    // world `linuxcontainers/alpine`), never the project itself. Must
    // stay unowned with a base_image_source note despite the name match.
    let eeee_row = r
        .unowned
        .iter()
        .find(|u| u.path_or_object == format!("{}:base-image-trap", fx.checkout_name))
        .expect("base-image-trap docker image unowned row");
    assert_eq!(
        eeee_row.reason,
        swamp_core::report::UnownedReason::DockerNoJoin
    );
    assert!(
        eeee_row
            .note
            .as_deref()
            .is_some_and(|n| n.contains("base_image_source")),
        "name-matching image with a foreign image.source must carry a \
         base_image_source note and stay unowned: {eeee_row:?}"
    );
    assert!(
        !checkout_project.worktrees.iter().any(|w| w
            .artifacts
            .iter()
            .any(|a| a.path.to_string_lossy().contains("base-image-trap"))),
        "the base-image-trap image must never be attributed to the \
         name-matching project"
    );

    // ffff: compose-project label matches no discovered project, but
    // working_dir is a path inside the linked worktree -> joined, High,
    // to the linked worktree (not the main checkout).
    let linked_worktree_row = checkout_project
        .worktrees
        .iter()
        .find(|w| w.path == fx.linked_worktree)
        .expect("linked worktree row");
    let ffff_row = linked_worktree_row
        .artifacts
        .iter()
        .find(|a| {
            a.path
                .to_string_lossy()
                .contains("fixture/working-dir-join")
        })
        .expect("working_dir-joined docker image row, under the linked worktree");
    assert_eq!(ffff_row.confidence, swamp_core::entities::Confidence::High);

    // gggg: compose-project label matches no discovered project and has
    // no other evidence -> unowned, with a compose_project note so a
    // later slice can still group it.
    let gggg_row = r
        .unowned
        .iter()
        .find(|u| {
            u.path_or_object
                .contains("fixture/unmatched-compose-project")
        })
        .expect("unmatched-compose-project docker image unowned row");
    assert_eq!(
        gggg_row.reason,
        swamp_core::report::UnownedReason::DockerNoJoin
    );
    assert!(
        gggg_row
            .note
            .as_deref()
            .is_some_and(|n| n.contains("compose_project=")),
        "an unmatched compose-project label must be recorded as a note: {gggg_row:?}"
    );

    // build-cache entry has no label source at all -> stays unowned.
    let build_cache_row = r
        .unowned
        .iter()
        .find(|u| u.path_or_object.contains("buildcache-no-evidence"))
        .expect("unowned build cache row");
    assert_eq!(
        build_cache_row.reason,
        swamp_core::report::UnownedReason::DockerNoJoin
    );

    // the volume's compose-project label matches the checkout directly
    // from `docker system df -v` output (volumes carry Labels there
    // already, unlike images) -> joined, High.
    let volume_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path.to_string_lossy() == "fixture-project-volume")
        .expect("compose-label-joined volume row");
    assert_eq!(
        volume_row.kind,
        swamp_core::report::ArtifactKind::DockerVolume
    );
    assert_eq!(
        volume_row.confidence,
        swamp_core::entities::Confidence::High
    );

    // 9999 (#28): compose-project label matches no discovered project's
    // own name, but the checkout root has a `compose.yaml` naming
    // `fixture-stack` -> joined, High, rule `compose_file_name`, to the
    // checkout's main worktree.
    let compose_name_row = main_worktree
        .artifacts
        .iter()
        .find(|a| {
            a.path
                .to_string_lossy()
                .contains("fixture/compose-file-name-join")
        })
        .expect("compose-file-name-joined docker image row");
    assert_eq!(
        compose_name_row.confidence,
        swamp_core::entities::Confidence::High
    );
    assert_eq!(compose_name_row.source.tool, "docker.compose_file_name");
    assert!(
        !fx.compose_project_name.is_empty() && fx.compose_file.exists(),
        "fixture must write a compose.yaml naming the project for this rule to match"
    );

    let compose_name_volume_row = main_worktree
        .artifacts
        .iter()
        .find(|a| a.path.to_string_lossy() == "fixture-stack-compose-name-volume")
        .expect("compose-file-name-joined volume row");
    assert_eq!(
        compose_name_volume_row.kind,
        swamp_core::report::ArtifactKind::DockerVolume
    );
    assert_eq!(
        compose_name_volume_row.confidence,
        swamp_core::entities::Confidence::High
    );
    assert_eq!(
        compose_name_volume_row.source.tool,
        "docker.compose_file_name"
    );

    // Docker bytes are tracked separately, never folded into the
    // filesystem walk's reconciliation.
    assert!(r.reconciliation.docker_attributed > 0);
    assert!(r.reconciliation.docker_unowned > 0);

    // (c) totals reconcile.
    assert_eq!(
        r.reconciliation.attributed + r.reconciliation.unowned,
        r.reconciliation.walked_total,
        "attributed + unowned must equal walked_total"
    );
}
