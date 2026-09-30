//! Markability for **bulk** marking (`A`, a project mark): which report
//! rows are a "folded unit" that one gesture may sweep up (dependency
//! trees, build outputs, caches, Docker objects). Space on a single row is
//! never limited by this: a checkout, a worktree, `.git` or a Source
//! directory marks on its own row with its warnings on the confirm
//! (`App::mark_row`); this list only keeps them out of a gesture that
//! would mark many at once.

use swamp_core::report::ArtifactKind;

/// A stable identity for a unit that can be marked, used as the key in
/// the app's mark-set and later to build the plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitId(pub String);

impl UnitId {
    pub fn for_artifact(path: &std::path::Path) -> Self {
        UnitId(path.display().to_string())
    }
}

/// Whether this artifact kind is a foldable unit Backspace may mark.
/// Returns `Err(reason)` naming why an unmarkable kind is refused, so the
/// caller can show it inline.
pub fn markable(kind: &ArtifactKind) -> Result<(), &'static str> {
    match kind {
        ArtifactKind::DependencyTree
        | ArtifactKind::BuildOutput
        | ArtifactKind::Cache
        | ArtifactKind::DockerImage
        | ArtifactKind::DockerVolume
        // A path under the root that no project claims: a unit with an
        // owner of nobody, not a non-unit. It goes to Trash like any
        // other path, and the confirm line says nothing claims it.
        | ArtifactKind::Loose => Ok(()),
        // Docker has no per-entry build-cache removal, so there is no
        // action to authorize for one record.
        ArtifactKind::DockerBuildCache => {
            Err("docker has no per-entry build-cache removal; `docker builder prune` acts on all of it")
        }
        ArtifactKind::Git => Err("git metadata is left out of mark-all; Space on its own row marks it, and the confirm says its history goes with it"),
        ArtifactKind::Source => Err("source trees are left out of mark-all; Space on its own row marks one"),
        // Bytes scattered across the checkout, reported under the
        // worktree's own path: there is no single directory to act on.
        ArtifactKind::Ignored | ArtifactKind::Untracked => Err(
            "an aggregate of every such path under the checkout, not one directory; open the worktree and act on what is inside it",
        ),
        ArtifactKind::Unknown => Err("kind is unclassified, so it is left out of mark-all; Space on its own row marks it"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_artifacts_are_markable() {
        for k in [
            ArtifactKind::DependencyTree,
            ArtifactKind::BuildOutput,
            ArtifactKind::Cache,
            ArtifactKind::DockerImage,
            ArtifactKind::DockerVolume,
            ArtifactKind::Loose,
        ] {
            assert!(markable(&k).is_ok(), "{k:?} should be markable");
        }
    }

    #[test]
    fn non_artifacts_are_refused_with_a_reason() {
        for k in [
            ArtifactKind::Git,
            ArtifactKind::Source,
            ArtifactKind::DockerBuildCache,
            ArtifactKind::Unknown,
        ] {
            let reason = markable(&k).unwrap_err();
            assert!(!reason.is_empty());
        }
    }
}
