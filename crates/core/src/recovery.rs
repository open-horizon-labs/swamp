//! Artifact-specific recovery evidence (#58), replacing the blanket
//! per-kind labels `actions::recovery_for` still uses for its plain-text
//! summary. `actions::recovery_for` is a hypothesis about a *kind*
//! (`DependencyTree` -> `network_fetch`); this module narrows that
//! hypothesis to a specific unit's actually-sourced evidence (a lockfile
//! that exists, a Maven `_remote.repositories` marker, a known manager
//! pin) and states what remains unknown rather than promising a backup
//! or a restore time nobody measured.
//!
//! A [`RecoveryAssessment`] is recomputed fresh from current facts every
//! time it is asked for; nothing here is cached or trusted past its own
//! call, so a changed/missing source is reflected immediately rather
//! than a stale recovery claim surviving past the fact that produced it.

use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};
use std::path::Path;

/// How a unit's content could plausibly be obtained again if the local
/// copy is gone. Never a promise -- see [`RecoveryAssessment::unresolved_unknowns`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryPath {
    /// Rebuildable from source already present locally (a build output
    /// whose worktree/source still exists).
    Rebuild,
    /// Re-obtainable over the network from a named, sourced origin (a
    /// dependency lockfile, a registry reference).
    NetworkFetch,
    /// Reinstallable from a local manager/cache without a network fetch
    /// (a toolchain version already downloaded elsewhere, a package the
    /// manager's own local cache still holds).
    LocalReinstall,
    /// State that may exist nowhere else once removed (mutable
    /// simulator/emulator/device data, a Docker volume with
    /// application-written data) -- directory category alone never
    /// proves this either way; a project build cache with no source
    /// signal lands here too, distinct from a definite rebuild.
    PotentiallyUniqueLocalState,
    /// Recoverable only if the user has an out-of-band backup; swamp
    /// cannot confirm one exists.
    BackupDependent,
    /// No sourced signal was available to narrow this at all.
    Unknown,
}

impl RecoveryPath {
    fn subtype(self) -> FactSubtype {
        match self {
            RecoveryPath::Rebuild => FactSubtype::Rebuild,
            RecoveryPath::NetworkFetch => FactSubtype::NetworkFetch,
            RecoveryPath::LocalReinstall => FactSubtype::LocalReinstall,
            RecoveryPath::PotentiallyUniqueLocalState => FactSubtype::PotentiallyUniqueLocalState,
            RecoveryPath::BackupDependent => FactSubtype::BackupDependent,
            RecoveryPath::Unknown => FactSubtype::UnknownPrerequisites,
        }
    }
}

/// One sourced fact backing (or failing to back) the chosen recovery
/// path -- e.g. "Cargo.lock present at `<path>`" -- never a bare claim.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecoveryPrerequisite {
    pub description: String,
    pub source: EvidenceSource,
    /// `None` when whether this prerequisite actually holds could not be
    /// checked (e.g. "registry is reachable" -- swamp does not probe the
    /// network to answer this).
    pub satisfied: Option<bool>,
}

/// The full recovery picture for one unit: chosen path, its supporting
/// evidence, prerequisites with their sources, unresolved material
/// unknowns, and (only when genuinely known) a sourced cost estimate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecoveryAssessment {
    pub path: RecoveryPath,
    pub evidence: Evidence,
    /// Whether swamp's own removal action moves this unit to Trash first
    /// (recoverable there until Trash is emptied) rather than a
    /// permanent daemon-side removal. Independent of `path` -- a Docker
    /// volume can be `NetworkFetch`-sourced storage (a pulled image
    /// layer) and still bypass Trash entirely.
    pub trash_available: bool,
    pub prerequisites: Vec<RecoveryPrerequisite>,
    /// Material unknowns a human would want answered before relying on
    /// `path` -- e.g. "registry/network availability at restore time is
    /// not checked". Never silently dropped in favor of a clean-looking
    /// summary.
    pub unresolved_unknowns: Vec<String>,
    /// A cost estimate, only when it is actually sourced (never a
    /// fabricated duration like "about 2 minutes").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_estimate: Option<String>,
    /// The smallest concrete check a human could run to resolve the
    /// biggest remaining unknown -- not a generic "verify before
    /// deleting" warning.
    pub follow_up_check: Option<String>,
}

fn now() -> u64 {
    crate::entities::now()
}

fn assessment(
    path: RecoveryPath,
    source: EvidenceSource,
    trash_available: bool,
    prerequisites: Vec<RecoveryPrerequisite>,
    unresolved_unknowns: Vec<String>,
    cost_estimate: Option<String>,
    follow_up_check: Option<String>,
) -> RecoveryAssessment {
    let observed_at = now();
    let evidence = Evidence::known(
        FactKind::Recovery,
        path.subtype(),
        FactValue::Text(format!("{path:?}")),
        source,
        observed_at,
    );
    RecoveryAssessment {
        path,
        evidence,
        trash_available,
        prerequisites,
        unresolved_unknowns,
        cost_estimate,
        follow_up_check,
    }
}

fn unknown_assessment(reason: impl Into<String>, trash_available: bool) -> RecoveryAssessment {
    let observed_at = now();
    let reason = reason.into();
    RecoveryAssessment {
        path: RecoveryPath::Unknown,
        evidence: Evidence::unknown(
            FactKind::Recovery,
            FactSubtype::UnknownPrerequisites,
            EvidenceSource::Inferred {
                basis: "no sourced restoration evidence found".into(),
            },
            observed_at,
            crate::evidence::Reason::carried(reason.clone()),
        ),
        trash_available,
        prerequisites: Vec::new(),
        unresolved_unknowns: vec![reason],
        cost_estimate: None,
        follow_up_check: Some(
            "inspect this unit directly (its manifest/metadata) to find a restoration source before removing it"
                .into(),
        ),
    }
}

/// A Cargo/npm/pnpm/Go-style dependency tree: `NetworkFetch` when a
/// lockfile is present and readable (the declared origin), `Unknown`
/// otherwise. Network/registry reachability at restore time is always
/// stated as a material unknown -- swamp never probes it.
pub fn dependency_tree_recovery(lockfile_path: Option<&Path>) -> RecoveryAssessment {
    match lockfile_path {
        Some(path) => assessment(
            RecoveryPath::NetworkFetch,
            EvidenceSource::Lockfile {
                ecosystem: "dependency-manifest".into(),
                path: path.display().to_string(),
            },
            true,
            vec![RecoveryPrerequisite {
                description: format!("lockfile present at {}", path.display()),
                source: EvidenceSource::Lockfile {
                    ecosystem: "dependency-manifest".into(),
                    path: path.display().to_string(),
                },
                satisfied: Some(true),
            }],
            vec![
                "registry/git remote availability and credentials at restore time are not checked"
                    .into(),
            ],
            None,
            Some(format!(
                "re-fetch into a scratch directory using {} to confirm every entry is still reachable before relying on this",
                path.display()
            )),
        ),
        None => unknown_assessment(
            "no lockfile found declaring this dependency tree's origin",
            true,
        ),
    }
}

/// A build output whose worktree/source is still present locally:
/// `Rebuild`. Absence of the source directory is `Unknown`, never
/// silently assumed rebuildable.
pub fn build_output_recovery(source_present: bool, source_path: &Path) -> RecoveryAssessment {
    if source_present {
        assessment(
            RecoveryPath::Rebuild,
            EvidenceSource::FilesystemMetadata {
                detail: format!("source present at {}", source_path.display()),
            },
            true,
            vec![RecoveryPrerequisite {
                description: format!("source checkout present at {}", source_path.display()),
                source: EvidenceSource::FilesystemMetadata {
                    detail: "worktree presence".into(),
                },
                satisfied: Some(true),
            }],
            vec!["build toolchain/dependency availability at rebuild time is not checked".into()],
            None,
            Some("run this project's own build once to confirm it still succeeds before relying on rebuildability".into()),
        )
    } else {
        unknown_assessment(
            format!(
                "no source checkout found at {} to rebuild from",
                source_path.display()
            ),
            true,
        )
    }
}

/// Maven's local repository mixes downloaded and locally-`mvn
/// install`ed artifacts with no directory-level signal
/// (`docs/locations.md`'s named limit). `has_remote_repositories_marker`
/// is the one per-artifact signal that narrows this: Maven writes a
/// `_remote.repositories` file next to an artifact fetched from a named
/// repository; its absence does not prove a local install (an older
/// Maven, or a repository that never wrote one), so that case still
/// reports `Unknown`, distinct from actively confirming a local-only
/// install some other way.
pub fn maven_artifact_recovery(has_remote_repositories_marker: bool) -> RecoveryAssessment {
    if has_remote_repositories_marker {
        assessment(
            RecoveryPath::NetworkFetch,
            EvidenceSource::BuildMetadata {
                path: "_remote.repositories".into(),
            },
            true,
            vec![RecoveryPrerequisite {
                description: "_remote.repositories marker names a remote repository origin"
                    .into(),
                source: EvidenceSource::BuildMetadata {
                    path: "_remote.repositories".into(),
                },
                satisfied: Some(true),
            }],
            vec![
                "named repository's availability/credentials at restore time are not checked"
                    .into(),
            ],
            None,
            Some(
                "read this artifact's _remote.repositories file to confirm which repository it names, then verify that repository is still reachable".into(),
            ),
        )
    } else {
        unknown_assessment(
            "no _remote.repositories marker found; directory location alone (Maven's local \
             repository) cannot distinguish a downloaded artifact from a locally-`mvn install`ed \
             one with no remote origin",
            true,
        )
    }
}

/// Mutable per-instance state (a simulator/emulator device's data
/// directory, a Docker volume with application-written data): always
/// `PotentiallyUniqueLocalState`, never assumed reproducible from any
/// image/snapshot swamp has not actually inspected.
pub fn mutable_environment_recovery(kind: &str, path_display: &str) -> RecoveryAssessment {
    assessment(
        RecoveryPath::PotentiallyUniqueLocalState,
        EvidenceSource::FilesystemMetadata {
            detail: format!("{kind} instance data at {path_display}"),
        },
        true,
        Vec::new(),
        vec![format!(
            "whether this {kind}'s current state is reproducible from a known base image/\
             snapshot is not checked; application-written data inside it may exist nowhere else"
        )],
        None,
        Some(format!(
            "check whether this {kind} was created from a snapshot/base image that still exists before removing its current state"
        )),
    )
}

/// A Docker volume: same treatment as any other mutable environment
/// unless the caller has independent evidence it is only ever a mirror
/// of build/pull output (not asserted here -- Docker volumes are
/// application-managed by design).
pub fn docker_volume_recovery(name: &str) -> RecoveryAssessment {
    let mut a = mutable_environment_recovery("Docker volume", name);
    a.trash_available = false; // Docker removals bypass Trash entirely (see actions::recovery_for).
    a
}

/// An installed toolchain version (a version manager's `installs/`
/// entry): `LocalReinstall` when the exact version string is known (the
/// manager can reinstall it without guessing), `Unknown` otherwise.
pub fn toolchain_installation_recovery(manager: &str, version: Option<&str>) -> RecoveryAssessment {
    match version {
        Some(v) => assessment(
            RecoveryPath::LocalReinstall,
            EvidenceSource::ToolReported {
                tool: manager.to_string(),
                detail: format!("known version {v}"),
            },
            true,
            vec![RecoveryPrerequisite {
                description: format!("exact version {v} known"),
                source: EvidenceSource::ToolReported {
                    tool: manager.to_string(),
                    detail: "installed-version record".into(),
                },
                satisfied: Some(true),
            }],
            vec![
                "network availability of this version's installer/download at reinstall time is not checked"
                    .into(),
            ],
            None,
            Some(format!(
                "run `{manager} install {v}` (or the equivalent) to confirm it can still be reinstalled"
            )),
        ),
        None => unknown_assessment(
            format!("no exact version string known for this {manager} installation"),
            true,
        ),
    }
}

/// A generated-output/cache unit with no source or lockfile signal at
/// all: distinct from a confirmed `Rebuild`/`NetworkFetch` -- category
/// (cache directory) alone never proves recoverability.
pub fn cache_without_signal_recovery(reason: impl Into<String>) -> RecoveryAssessment {
    unknown_assessment(reason, true)
}

/// A Docker image (#58): the generic per-row evidence pass knows only
/// whether it recorded a repository tag and whether this row was joined
/// to a known project's worktree (compose/Dockerfile source potentially
/// present) -- not which of "build:" or "image:" a compose file uses,
/// so this deliberately never asserts one specific path. `dangling`
/// (no repository tag recorded at all) leaves even the *tag* origin
/// unknown; a tagged image joined to a project names both candidate
/// prerequisites explicitly rather than picking one.
pub fn docker_image_recovery(
    repo_tag: Option<&str>,
    joined_to_project: bool,
) -> RecoveryAssessment {
    let observed_at = now();
    let source = EvidenceSource::DockerApi {
        detail: "image inspect".into(),
    };
    match repo_tag {
        None => RecoveryAssessment {
            path: RecoveryPath::Unknown,
            evidence: Evidence::unknown(
                FactKind::Recovery,
                FactSubtype::UnknownPrerequisites,
                source,
                observed_at,
                crate::reason!("dangling image: no repository tag recorded to name a pull source, and no build source is recorded here"),
            ),
            trash_available: false,
            prerequisites: Vec::new(),
            unresolved_unknowns: vec![
                "neither a pull origin nor a build definition is recorded for this image".into(),
            ],
            cost_estimate: None,
            follow_up_check: Some(
                "run `docker image inspect` for this image's id to look for a build history or base-image origin before removing it".into(),
            ),
        },
        Some(tag) => {
            let mut unresolved = vec![
                "whether this image is defined by a compose/Dockerfile `build:` step (rebuildable) or an `image:` pull reference is not distinguished from the tag alone".into(),
            ];
            if joined_to_project {
                unresolved.push(
                    "if rebuildable, build toolchain/base-image availability at rebuild time is not checked".into(),
                );
            } else {
                unresolved.push(
                    "if pulled, registry availability/credentials at restore time are not checked"
                        .into(),
                );
            }
            RecoveryAssessment {
                path: RecoveryPath::Unknown,
                evidence: Evidence::unknown(
                    FactKind::Recovery,
                    FactSubtype::UnknownPrerequisites,
                    source,
                    observed_at,
                    crate::reason!("tag '{tag}' recorded, but pull-vs-rebuild origin is not distinguished"),
                ),
                trash_available: false,
                prerequisites: vec![RecoveryPrerequisite {
                    description: format!("repository tag '{tag}' recorded"),
                    source: EvidenceSource::DockerApi {
                        detail: "image inspect".into(),
                    },
                    satisfied: Some(true),
                }],
                unresolved_unknowns: unresolved,
                cost_estimate: None,
                follow_up_check: Some(if joined_to_project {
                    format!(
                        "check this project's compose file/Dockerfile for a `build:` step naming '{tag}', or try `docker pull {tag}` to confirm a registry origin"
                    )
                } else {
                    format!("try `docker pull {tag}` to confirm this image is still available from a registry")
                }),
            }
        }
    }
}

/// Docker build cache (#58): BuildKit cache is only ever locally
/// generated from a build; `Rebuild` when the worktree that produced it
/// is still present in this report (mirrors [`build_output_recovery`]'s
/// own source-presence logic), `Unknown` otherwise. Never a promise that
/// a rebuild reproduces the cache bit-for-bit.
pub fn docker_build_cache_recovery(source_present: bool) -> RecoveryAssessment {
    if source_present {
        assessment(
            RecoveryPath::Rebuild,
            EvidenceSource::DockerApi {
                detail: "build cache entry joined to a present worktree".into(),
            },
            false,
            vec![RecoveryPrerequisite {
                description: "joined project's worktree is present in this report".into(),
                source: EvidenceSource::FilesystemMetadata {
                    detail: "worktree presence".into(),
                },
                satisfied: Some(true),
            }],
            vec![
                "BuildKit cache reconstruction is best-effort; a rebuild is not guaranteed to reproduce this exact cache layer".into(),
            ],
            None,
            Some("run this project's `docker build` again to confirm the cache layer regenerates".into()),
        )
    } else {
        unknown_assessment(
            "no joined project worktree present in this report to rebuild this cache entry from",
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_lockfile_is_unknown_not_assumed_network_fetch() {
        let a = dependency_tree_recovery(None);
        assert_eq!(a.path, RecoveryPath::Unknown);
        assert!(!a.unresolved_unknowns.is_empty());
        assert!(a.follow_up_check.is_some());
    }

    #[test]
    fn present_lockfile_states_network_fetch_with_reachability_unknown() {
        let a = dependency_tree_recovery(Some(Path::new("/proj/Cargo.lock")));
        assert_eq!(a.path, RecoveryPath::NetworkFetch);
        // The tempting shortcut this rejects: claiming NetworkFetch
        // *works*, i.e. asserting the registry is actually reachable.
        assert!(
            a.unresolved_unknowns
                .iter()
                .any(|u| u.contains("reachable") || u.contains("credentials"))
        );
        assert!(a.cost_estimate.is_none());
    }

    #[test]
    fn private_or_offline_dependency_still_names_a_concrete_follow_up_check() {
        let a = dependency_tree_recovery(Some(Path::new("/proj/package-lock.json")));
        let check = a.follow_up_check.unwrap();
        assert!(check.contains("package-lock.json"));
    }

    #[test]
    fn local_only_maven_artifact_without_marker_is_unknown() {
        let a = maven_artifact_recovery(false);
        assert_eq!(a.path, RecoveryPath::Unknown);
        assert!(
            a.unresolved_unknowns[0].contains("_remote.repositories"),
            "must name why: directory category alone is not proof"
        );
    }

    #[test]
    fn maven_artifact_with_marker_is_network_fetch() {
        let a = maven_artifact_recovery(true);
        assert_eq!(a.path, RecoveryPath::NetworkFetch);
    }

    #[test]
    fn mutable_emulator_state_is_potentially_unique_never_rebuild() {
        let a = mutable_environment_recovery("Android emulator", "/avd/Pixel_6/data");
        assert_eq!(a.path, RecoveryPath::PotentiallyUniqueLocalState);
        assert!(a.cost_estimate.is_none());
    }

    #[test]
    fn docker_volume_recovery_has_no_trash_available() {
        let a = docker_volume_recovery("pgdata");
        assert_eq!(a.path, RecoveryPath::PotentiallyUniqueLocalState);
        assert!(!a.trash_available);
    }

    #[test]
    fn stale_recovery_facts_are_recomputed_not_cached() {
        // Simulate a source appearing between two calls: nothing in this
        // module caches a prior result, so the same inputs never differ
        // from a fresh recomputation, and a *changed* input (the
        // lockfile now missing) immediately changes the answer rather
        // than trusting an old assessment.
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("Cargo.lock");
        std::fs::write(&lock, b"# lock").unwrap();
        let present = dependency_tree_recovery(Some(&lock));
        assert_eq!(present.path, RecoveryPath::NetworkFetch);
        std::fs::remove_file(&lock).unwrap();
        // The caller is responsible for re-checking existence before
        // calling again (this module does no I/O of its own for the
        // lockfile path); demonstrate that doing so changes the result.
        let now_missing: Option<PathBuf> = lock.exists().then_some(lock.clone());
        let after = dependency_tree_recovery(now_missing.as_deref());
        assert_eq!(after.path, RecoveryPath::Unknown);
    }

    #[test]
    fn never_fabricates_a_cost_estimate() {
        for a in [
            dependency_tree_recovery(Some(Path::new("/x/Cargo.lock"))),
            build_output_recovery(true, Path::new("/x")),
            maven_artifact_recovery(true),
            toolchain_installation_recovery("rustup", Some("1.82.0")),
        ] {
            assert!(a.cost_estimate.is_none(), "{:?} fabricated a cost", a.path);
        }
    }

    #[test]
    fn toolchain_reinstall_needs_exact_version() {
        let known = toolchain_installation_recovery("nvm", Some("20.11.0"));
        assert_eq!(known.path, RecoveryPath::LocalReinstall);
        let unknown = toolchain_installation_recovery("nvm", None);
        assert_eq!(unknown.path, RecoveryPath::Unknown);
    }

    #[test]
    fn dangling_docker_image_never_asserts_a_pull_or_rebuild_path() {
        let a = docker_image_recovery(None, false);
        assert_eq!(a.path, RecoveryPath::Unknown);
        assert!(!a.trash_available);
        assert!(a.follow_up_check.is_some());
    }

    #[test]
    fn tagged_docker_image_names_both_candidate_origins_never_picks_one() {
        // The tempting shortcut this rejects: assuming a tagged image
        // joined to a project must be locally rebuildable via compose
        // `build:`, when the tag could equally be a pulled `image:`
        // reference (e.g. `postgres:16`).
        let joined = docker_image_recovery(Some("myapp:latest"), true);
        assert_eq!(joined.path, RecoveryPath::Unknown);
        assert!(
            joined
                .unresolved_unknowns
                .iter()
                .any(|u| u.contains("build:") || u.contains("image:"))
        );
        let unowned = docker_image_recovery(Some("postgres:16"), false);
        assert_eq!(unowned.path, RecoveryPath::Unknown);
        assert!(unowned.follow_up_check.unwrap().contains("docker pull"));
    }

    #[test]
    fn docker_build_cache_rebuild_needs_a_present_worktree() {
        let present = docker_build_cache_recovery(true);
        assert_eq!(present.path, RecoveryPath::Rebuild);
        assert!(!present.trash_available);
        let absent = docker_build_cache_recovery(false);
        assert_eq!(absent.path, RecoveryPath::Unknown);
    }

    #[test]
    fn docker_recovery_never_fabricates_a_cost_estimate() {
        for a in [
            docker_image_recovery(Some("myapp:latest"), true),
            docker_image_recovery(None, false),
            docker_build_cache_recovery(true),
        ] {
            assert!(a.cost_estimate.is_none(), "{:?} fabricated a cost", a.path);
        }
    }
}
