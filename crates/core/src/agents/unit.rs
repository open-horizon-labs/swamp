//! The unit an adapter produces, and the one way to build it.

/// [`CandidateAgentUnit::into_parts`].
pub(super) struct CandidateParts {
    pub(super) category: AgentCategory,
    pub(super) relative_path: String,
    pub(super) path: PathBuf,
    pub(super) members: Vec<AgentMember>,
    pub(super) bytes: u64,
    pub(super) mtime_max: u64,
    pub(super) project_link: ProjectLinkState,
    pub(super) action: AgentActionCapability,
    pub(super) note: Option<String>,
    pub(super) complete: bool,
}

use super::{
    AgentActionCapability, AgentCategory, AgentMember, LinkBasis, ProjectLinkState, relative_to,
    resolve_declared_workspace,
};
use std::path::{Path, PathBuf};

/// What an adapter (`claude_code::identify`) produces before growth
/// history and human-protect intent are layered on by
/// [`discover_and_measure`]. Never serialized on its own; exists so an
/// adapter never has to fabricate `growth_bytes`/`regrowth_count`.
///
/// Every field that decides what may happen to the unit is private to
/// this module (`.oh/guardrails/agent-units-built-through-builder.md`):
/// an adapter reads a unit through its accessors and builds one only
/// through [`AgentUnitBuilder`], so protected-by-default cannot be
/// lifted by assignment -- `u.protected = false`, or `let f = &mut
/// u.protected; *f = false;` -- anywhere outside this file. `path` and
/// `note` stay public fields: they decide nothing, and the reviewers'
/// counterexample files read them as fields.
#[derive(Debug)]
pub struct CandidateAgentUnit {
    category: AgentCategory,
    relative_path: String,
    pub path: PathBuf,
    members: Vec<AgentMember>,
    bytes: u64,
    mtime_max: u64,
    protected: bool,
    protect_reason: Option<String>,
    project_link: ProjectLinkState,
    action: AgentActionCapability,
    pub note: Option<String>,
    /// How `project_link` was arrived at, so container-level reuse can
    /// tell a fact that lives inside the container from one that does
    /// not. See [`LinkBasis`].
    link_basis: LinkBasis,
    /// `false` when `bytes` is a lower bound rather than this unit's true
    /// size -- a bounded fold
    /// ([`crate::folded_measurement::folded_bytes_bounded_stamped`])
    /// that hit its entry cap or an unreadable subdirectory a few levels
    /// in. Never lifted back to `true` by anything outside this module
    /// ([`AgentUnitBuilder::incomplete`] is the only way to clear it):
    /// the caller that turns this into an [`super::AgentUnit`] must
    /// protect it from a growth/regrowth delta the same way
    /// `external.rs` protects an unreadable unit
    /// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    complete: bool,
}

impl CandidateAgentUnit {
    pub fn category(&self) -> AgentCategory {
        self.category
    }
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn members(&self) -> &[AgentMember] {
        &self.members
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn mtime_max(&self) -> u64 {
        self.mtime_max
    }
    pub fn protected(&self) -> bool {
        self.protected
    }
    pub fn protect_reason(&self) -> Option<&str> {
        self.protect_reason.as_deref()
    }
    pub fn project_link(&self) -> &ProjectLinkState {
        &self.project_link
    }
    pub fn action(&self) -> AgentActionCapability {
        self.action
    }
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
    pub fn link_basis(&self) -> &LinkBasis {
        &self.link_basis
    }
    /// `false` means `bytes` is a partial sum this pass, never this
    /// unit's confirmed size -- see the field doc on
    /// [`CandidateAgentUnit::complete`].
    pub fn complete(&self) -> bool {
        self.complete
    }

    /// Nests this unit's relative path under `prefix` (a host label).
    pub fn prefix_relative_path(&mut self, prefix: &str) {
        self.relative_path = format!("{prefix}/{}", self.relative_path);
    }

    /// For a tool whose modeled layout the support matrix could not
    /// verify: no action and no project linkage are offered, and the note
    /// says why. Only ever narrows what the unit claims.
    pub(super) fn withdraw_for_unverified_layout(&mut self, tool_name: &str) {
        self.action = AgentActionCapability::None;
        if !matches!(self.project_link, ProjectLinkState::NotApplicable) {
            self.project_link = ProjectLinkState::Unresolved {
                reason: format!(
                    "{tool_name}'s storage layout is not confirmed against its own source or \
                     documentation (support level: unverified), so a project link would rest \
                     on an unverified layout"
                ),
            };
        }
        let why = "support level: unverified -- this tool's layout could not be confirmed \
                   against its own source or documentation, so no action is offered";
        self.note = Some(match self.note.take() {
            Some(n) => format!("{n}; {why}"),
            None => why.to_string(),
        });
    }

    /// A unit replayed from the container store exactly as a previous
    /// pass built it (through the builder): the one constructor besides
    /// [`AgentUnitBuilder`]. Only the container cache in `agents/mod.rs`
    /// calls it (the gate audit rejects it in an adapter).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn replayed(
        category: AgentCategory,
        relative_path: String,
        path: PathBuf,
        bytes: u64,
        mtime_max: u64,
        protected: bool,
        protect_reason: Option<String>,
        project_link: ProjectLinkState,
        action: AgentActionCapability,
        note: Option<String>,
        link_basis: LinkBasis,
    ) -> CandidateAgentUnit {
        CandidateAgentUnit {
            category,
            relative_path,
            path,
            members: Vec::new(),
            bytes,
            mtime_max,
            protected,
            protect_reason,
            project_link,
            action,
            note,
            link_basis,
            // Only a complete container is ever stored (`unstorable` in
            // `agents/mod.rs::container_with_facts`, set whenever a
            // fold inside it came back truncated), so a replayed unit
            // was complete when it was recorded -- the exact precedent
            // `folded_measurement::reuse_folded_measurement` states for
            // the external-unit case.
            complete: true,
        }
    }

    /// Re-resolves a replayed unit's declared project link live (a
    /// [`LinkBasis::Declared`] link is never replayed as stored).
    pub(super) fn set_resolved_link(&mut self, link: ProjectLinkState) {
        self.project_link = link;
    }

    /// Replaces a cached declaration with fresh adapter metadata, then
    /// lets the shared container resolver apply worktree inference. The
    /// adapter never sets a resolved link directly.
    pub(super) fn set_declared_project_link(
        &mut self,
        declared: Option<String>,
        missing_reason: &str,
    ) {
        self.project_link = resolve_declared_workspace(&declared, &[], missing_reason);
        self.link_basis = LinkBasis::Declared {
            declared,
            additional: Vec::new(),
            missing_reason: missing_reason.to_string(),
            folder_slug: None,
        };
    }

    /// Every field, by value, for the one place that turns a candidate
    /// into the delivered [`super::AgentUnit`] (after protection has been
    /// layered on).
    pub(super) fn into_parts(self) -> CandidateParts {
        CandidateParts {
            category: self.category,
            relative_path: self.relative_path,
            path: self.path,
            members: self.members,
            bytes: self.bytes,
            mtime_max: self.mtime_max,
            project_link: self.project_link,
            action: self.action,
            note: self.note,
            complete: self.complete,
        }
    }

    /// A member of a [`CandidateAgentUnit::replayed`] unit.
    pub(super) fn push_replayed_member(&mut self, member: AgentMember) {
        self.members.push(member);
    }
}

// ---------------------------------------------------------------------
// AgentUnitBuilder: protected-by-default is a constructor, not a habit
// ---------------------------------------------------------------------

/// The only way an adapter builds a unit
/// (`.oh/guardrails/agent-units-built-through-builder.md`).
///
/// A `CandidateAgentUnit { .. }` literal has to spell out `protected`
/// and `protect_reason`, which means a new adapter can silently ship a
/// credentials file with `protected: false` and nothing notices. The
/// constructor applies [`AgentCategory::default_protected`] instead, and
/// lifting it requires [`AgentUnitBuilder::unprotect_with_reason`] --
/// visible in review, and in the diff.
pub struct AgentUnitBuilder {
    unit: CandidateAgentUnit,
}

impl AgentUnitBuilder {
    pub fn new(_tool_id: &str, category: AgentCategory, path: PathBuf) -> Self {
        let (protected, protect_reason) = if category.default_protected() {
            (
                true,
                Some(format!(
                    "{} is protected by default (credentials/config/skills/automation)",
                    category.label()
                )),
            )
        } else {
            (false, None)
        };
        let relative_path = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            unit: CandidateAgentUnit {
                category,
                relative_path,
                path,
                members: Vec::new(),
                bytes: 0,
                mtime_max: 0,
                protected,
                protect_reason,
                project_link: ProjectLinkState::NotApplicable,
                action: AgentActionCapability::None,
                note: None,
                link_basis: LinkBasis::Fixed,
                complete: true,
            },
        }
    }

    /// Sets `relative_path` from this unit's path relative to `home`,
    /// forward-slashed.
    pub fn relative_to(mut self, home: &Path) -> Self {
        self.unit.relative_path = relative_to(home, &self.unit.path);
        self
    }

    pub fn relative_path(mut self, rel: impl Into<String>) -> Self {
        self.unit.relative_path = rel.into();
        self
    }

    pub fn bytes(mut self, bytes: u64) -> Self {
        self.unit.bytes = bytes;
        self
    }

    pub fn mtime_max(mut self, mtime: u64) -> Self {
        self.unit.mtime_max = mtime;
        self
    }

    /// Sets the member list and derives `bytes` from it.
    pub fn members(mut self, members: Vec<AgentMember>) -> Self {
        self.unit.bytes = members.iter().map(|m| m.bytes).sum();
        self.unit.members = members;
        self
    }

    /// Sets the member list without touching an explicitly set byte
    /// total (a folded category unit whose members are a subset).
    pub fn members_keep_bytes(mut self, members: Vec<AgentMember>) -> Self {
        self.unit.members = members;
        self
    }

    pub fn project_link(mut self, link: ProjectLinkState) -> Self {
        self.unit.project_link = link;
        self.unit.link_basis = LinkBasis::Fixed;
        self
    }

    /// Resolves a path the tool itself declared, and records that this
    /// is where the link came from, so a reused container re-resolves it
    /// live instead of replaying a state that may since have gone stale
    /// ([`LinkBasis`]). The only form of linkage a container may be
    /// reused around.
    pub fn project_link_declared(self, declared: Option<String>, missing_reason: &str) -> Self {
        self.project_link_declared_workspace(declared, Vec::new(), missing_reason)
    }

    /// [`Self::project_link_declared`] for a unit that declares several
    /// workspace roots. The resolved state widens to
    /// [`ProjectLinkState::Shared`] when they resolve to different
    /// projects, and every declared path is stored so a replayed
    /// container re-resolves all of them rather than replaying a
    /// widening that may since have stopped being true.
    pub fn project_link_declared_workspace(
        mut self,
        declared: Option<String>,
        additional: Vec<String>,
        missing_reason: &str,
    ) -> Self {
        self.unit.project_link = resolve_declared_workspace(&declared, &additional, missing_reason);
        self.unit.link_basis = LinkBasis::Declared {
            declared,
            additional,
            missing_reason: missing_reason.to_string(),
            folder_slug: None,
        };
        self
    }

    /// [`Self::project_link_declared`] for a tool that keys its storage
    /// by an encoding of the workspace path (Claude Code's
    /// `projects/<slug>`). The slug is recorded beside the declared
    /// path; when the declared path is absent or cannot resolve, the
    /// shared layer may infer the link from it -- exactly one known
    /// worktree re-encoding to the slug -- and says so
    /// ([`super::LinkSource::Inferred`]). A successfully resolved
    /// declared path always wins. The adapter never decodes the slug and
    /// never matches a basename.
    pub fn project_link_declared_or_folder(
        mut self,
        declared: Option<String>,
        missing_reason: &str,
        folder_slug: String,
    ) -> Self {
        self.unit.project_link = resolve_declared_workspace(&declared, &[], missing_reason);
        self.unit.link_basis = LinkBasis::Declared {
            declared,
            additional: Vec::new(),
            missing_reason: missing_reason.to_string(),
            folder_slug: Some(folder_slug),
        };
        self
    }

    pub fn action(mut self, action: AgentActionCapability) -> Self {
        self.unit.action = action;
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.unit.note = Some(note.into());
        self
    }

    /// Marks this unit's `bytes` as a lower bound rather than its
    /// confirmed size -- a bounded fold that hit its entry cap or an
    /// unreadable subdirectory a few levels in. `reason` is folded into
    /// the unit's note (appended to whatever it already had, same as
    /// [`CandidateAgentUnit::withdraw_for_unverified_layout`]) so a human
    /// sees why the total may be an undercount; the caller in
    /// `agents::discover_and_measure_in` also reads
    /// [`CandidateAgentUnit::complete`] directly to keep this pass's
    /// partial total out of growth/regrowth history
    /// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
    pub fn incomplete(mut self, reason: impl Into<String>) -> Self {
        self.unit.complete = false;
        let reason = reason.into();
        self.unit.note = Some(match self.unit.note.take() {
            Some(n) => format!("{n}; {reason}"),
            None => reason,
        });
        self
    }

    /// Protects this unit for a stated reason, on top of whatever its
    /// category already implies.
    pub fn protect(mut self, reason: impl Into<String>) -> Self {
        self.unit.protected = true;
        self.unit.protect_reason = Some(reason.into());
        self
    }

    /// Lifts a category's default protection. Deliberately noisy: a
    /// reason is required and the audit records every use.
    pub fn unprotect_with_reason(mut self, reason: impl Into<crate::evidence::Reason>) -> Self {
        let reason: crate::evidence::Reason = reason.into();
        self.unit.protected = false;
        self.unit.protect_reason = Some(format!("default protection lifted: {reason}"));
        self
    }

    pub fn build(self) -> CandidateAgentUnit {
        self.unit
    }
}
