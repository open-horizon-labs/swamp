//! Current-state decision-evidence contract (#53).
//!
//! Consumers, activity, current-use, recovery and reclaimability are
//! different questions with different answers; a single "last used" field
//! or a safe/unsafe verdict cannot carry them. This module is the shared
//! shape every domain-evidence source (#54-#59) fills in and every
//! presentation layer (#60) reads, attached to artifact rows, external
//! units, agent units and nested build-artifact units through the report.
//!
//! Hard rules this type exists to enforce (see
//! `.oh/guardrails/activity-and-consumer-evidence-have-limits.md` and
//! `.oh/guardrails/coverage-changes-are-not-storage-changes.md`):
//!
//! - A fact always carries its [`EvidenceSource`] (what produced it) and
//!   its [`Freshness`]/coverage limit -- never a bare value with no
//!   provenance.
//! - "Not observed" is [`FactStatus::Unknown`] or [`FactStatus::Unavailable`],
//!   never silently omitted and never collapsed into "unused" or "safe".
//!   [`FactStatus::Conflicting`] exists so two sources disagreeing is a
//!   visible fact, not a coin flip.
//! - `event_at` (when the underlying thing happened) is always distinct
//!   from `observed_at` (when swamp looked). A filesystem mtime observed
//!   today about a file written last year has `event_at` last year.
//! - Evidence lives on current-state report/unit rows (this module), never
//!   in the byte-history Parquet store (`growth.rs`). Refreshing evidence
//!   alone must never create a byte-history delta or tombstone -- see
//!   `crates/core/tests/evidence_contract.rs`'s
//!   `refreshing_evidence_never_writes_byte_history_delta`.
//! - Invalidation is explicit: [`Evidence::invalidated_for`] names the
//!   source/identity/config change that makes a previously-collected fact
//!   no longer trustworthy, rather than silently keeping stale evidence or
//!   silently rewriting historical attribution.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Why a fact is `Unknown` or `Unavailable`. Never blank
/// (`.oh/guardrails/activity-and-consumer-evidence-have-limits.md`):
/// "not observed" with nothing saying why is the absence the contract
/// exists to rule out.
///
/// Made three ways, none of which accepts an arbitrary string:
///
/// * [`reason!`](crate::reason) with a literal (or a format string and
///   its arguments) -- checked non-blank **at compile time**;
/// * [`Reason::fixed`] for a `&'static str` held in a table or an enum;
/// * [`Reason::carried`] for a reason another fact already computed.
///
/// The last two check at run time and substitute an explicit
/// "not recorded" text for a blank one (and fail a debug build). There is
/// no `From<String>` and, outside the `testing` feature, no
/// `From<&str>`: `Evidence::unknown(.., String::from(""))` or a blank
/// `const` passed straight in does not compile, and neither does a
/// [`FactStatus::Unknown`] literal built with a plain string -- its
/// `reason` field is a `Reason` too.
///
/// Serialized as its text; deserializing a blank text yields the same
/// explicit "not recorded" text a blank runtime reason does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason(Cow<'static, str>);

impl Serialize for Reason {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Reason, D::Error> {
        let text = String::deserialize(d)?;
        Ok(if reason_is_blank(&text) {
            Reason(Cow::Borrowed(NOT_RECORDED))
        } else {
            Reason(Cow::Owned(text))
        })
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for Reason {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for Reason {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for Reason {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

const NOT_RECORDED: &str = "the reason for this was not recorded (a swamp bug: every Unknown or Unavailable fact must say why)";

/// Whether `s` is empty or only whitespace, in a `const` context.
#[doc(hidden)]
pub const fn reason_is_blank(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_whitespace() {
            return false;
        }
        i += 1;
    }
    true
}

impl Reason {
    /// The `reason!` macro's constructor for a formatted reason whose
    /// template was checked non-blank at compile time.
    #[doc(hidden)]
    pub fn __from_checked_format(s: String) -> Reason {
        // The template was non-blank; its arguments can still format to
        // nothing (`reason!("{}", x)` with an empty `x`).
        if reason_is_blank(&s) {
            return Reason(Cow::Borrowed(NOT_RECORDED));
        }
        Reason(Cow::Owned(s))
    }

    /// A reason held in a table or enum (`&'static str`), checked at run
    /// time.
    pub fn fixed(s: &'static str) -> Reason {
        debug_assert!(!reason_is_blank(s), "blank evidence reason");
        if reason_is_blank(s) {
            return Reason(Cow::Borrowed(NOT_RECORDED));
        }
        Reason(Cow::Borrowed(s))
    }

    /// A reason another fact already computed (a refusal text, an
    /// estimate's own reason), checked at run time.
    pub fn carried(s: impl Into<String>) -> Reason {
        let s = s.into();
        debug_assert!(!reason_is_blank(&s), "blank evidence reason");
        if reason_is_blank(&s) {
            return Reason(Cow::Borrowed(NOT_RECORDED));
        }
        Reason(Cow::Owned(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Reason> for String {
    fn from(r: Reason) -> String {
        r.0.into_owned()
    }
}

/// Test fixtures pass plain literals (`testing` feature only, which no
/// production build enables).
#[cfg(any(test, feature = "testing"))]
impl From<&'static str> for Reason {
    fn from(s: &'static str) -> Reason {
        Reason::fixed(s)
    }
}

/// A [`Reason`], checked non-blank at compile time:
/// `reason!("no lock file present")`, or
/// `reason!("could not stat path: {e}")` / `reason!("{} matches none", x)`.
#[macro_export]
macro_rules! reason {
    ($fmt:literal $(, $($arg:tt)*)?) => {{
        const _: () = assert!(
            !$crate::evidence::reason_is_blank($fmt),
            "an evidence reason must say why the fact is not known"
        );
        $crate::evidence::Reason::__from_checked_format(format!($fmt $(, $($arg)*)?))
    }};
}

/// The five decision-evidence domains this contract distinguishes.
/// Corresponds to #54 (Activity), #56/#57 (Consumer), #55 (CurrentUse),
/// #58 (Recovery) and #59 (Reclaimability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FactKind {
    Activity,
    Consumer,
    CurrentUse,
    Recovery,
    Reclaimability,
}

/// A finer-grained subtype named explicitly so a reader never has to
/// guess what a bare "activity" fact actually measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FactSubtype {
    // Activity (#54)
    /// Newest recorded modification among the unit's measured children
    /// (the folded walk's own stat, never re-labelled "last used").
    Modified,
    /// A filesystem access-time read, only when the mount is known not
    /// to suppress atime updates.
    Accessed,
    /// A tool's own reported use/build/execution timestamp (Docker
    /// `LastUsedAt`, a Cargo fingerprint mtime, an agent session mtime).
    ToolReportedUse,

    // Consumer (#56, #57)
    DeclaredConsumer,
    InferredConsumer,

    // CurrentUse (#55)
    Process,
    OpenFile,
    Lock,
    RunningContainer,
    Mounted,
    Booted,

    // Recovery (#58)
    Rebuild,
    NetworkFetch,
    LocalReinstall,
    TrashRecovery,
    BackupDependent,
    PotentiallyUniqueLocalState,
    UnknownPrerequisites,

    // Reclaimability (#59)
    LogicalBytes,
    AllocatedBytes,
    EstimatedReclaimable,
    ObservedFreed,
}

/// Where a fact came from, with enough provenance detail that a reader
/// can independently judge how much to trust it -- never a bare string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case")]
pub enum EvidenceSource {
    /// Read from the existing folded-walk stats (mtime/atime/size) with
    /// no additional per-file work.
    FilesystemMetadata { detail: String },
    /// A tool's own reported fact (Docker's daemon, a Cargo fingerprint
    /// file, an agent tool's session store), named explicitly.
    ToolReported { tool: String, detail: String },
    /// A bounded, read-only process/open-file query (`lsof`, a manager
    /// lock file check).
    ProcessQuery { tool: String },
    /// A manager-specific lock file's presence/holder.
    ManagerLock { tool: String, path: String },
    /// A project/tool configuration file declaring a reference (a
    /// version-manager file, `.tool-versions`, a global default file).
    ConfigDeclaration { path: String },
    /// A dependency lockfile establishing a declared (not runtime-proven)
    /// reference.
    Lockfile { ecosystem: String, path: String },
    /// Build-system metadata (Xcode `info.plist`, a Gradle lockfile).
    BuildMetadata { path: String },
    /// The Docker daemon API/inspect output.
    DockerApi { detail: String },
    /// `statvfs`/`statfs` free-space accounting.
    Statvfs,
    /// Derived from other facts already in this contract, named so it is
    /// never confused with a direct observation.
    Inferred { basis: String },
}

/// The value carried by a fact, once its status is [`FactStatus::Known`]
/// (or as one candidate inside [`FactStatus::Conflicting`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum FactValue {
    Timestamp(u64),
    Bool(bool),
    Text(String),
    Bytes(u64),
    SignedBytes(i64),
    Count(u64),
    List(Vec<String>),
}

/// A fact's resolution. Every non-known outcome is explicit and carries a
/// reason -- there is no variant that means "just leave this out".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum FactStatus {
    Known(FactValue),
    /// Not established: the source was consulted but could not say
    /// (empty/absent data, out of scanned scope). Never displayed or
    /// treated as "unused"/"safe".
    Unknown {
        reason: Reason,
    },
    /// The source itself could not be consulted this pass (permission
    /// denied, daemon unreachable, query timed out). Distinct from
    /// `Unknown`: here the *source* failed, not just the answer.
    Unavailable {
        reason: Reason,
    },
    /// Two or more sources disagree; every candidate is kept rather than
    /// silently picking one.
    Conflicting {
        candidates: Vec<FactValue>,
        reason: Reason,
    },
}

/// Freshness/expiry and coverage limits every fact must state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    /// Seconds after `observed_at` after which this fact should be
    /// rechecked rather than trusted as current (short-lived facts like a
    /// process/lock query). `None` means no defined expiry -- typically a
    /// filesystem read that is re-taken fresh on every report anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_after_secs: Option<u64>,
    /// A stated coverage limitation in words, e.g. "only measured
    /// children the folded walk recorded" or "scanned roots only; a
    /// reference from outside scanned scope would not appear here".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_note: Option<String>,
}

impl Freshness {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn expires_after(secs: u64) -> Self {
        Self {
            expires_after_secs: Some(secs),
            coverage_note: None,
        }
    }
    pub fn with_coverage(note: impl Into<String>) -> Self {
        Self {
            expires_after_secs: None,
            coverage_note: Some(note.into()),
        }
    }
    pub fn expires_after_with_coverage(secs: u64, note: impl Into<String>) -> Self {
        Self {
            expires_after_secs: Some(secs),
            coverage_note: Some(note.into()),
        }
    }
}

/// One decision-relevant fact, attached to an artifact row, external
/// unit, agent unit or nested build-artifact unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: FactKind,
    pub subtype: FactSubtype,
    pub status: FactStatus,
    pub source: EvidenceSource,
    /// When swamp observed/collected this fact (always present).
    pub observed_at: u64,
    /// When the underlying event happened, if known and different from
    /// `observed_at` (e.g. a file's mtime, a container's start time).
    /// `None` when no distinct event time is established (e.g. a live
    /// process-occupancy check: observing *is* the event).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_at: Option<u64>,
    #[serde(default, skip_serializing_if = "is_default_freshness")]
    pub freshness: Freshness,
    /// A short human-readable elaboration, never a verdict word ("safe",
    /// "unused", "stale-and-removable").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn is_default_freshness(f: &Freshness) -> bool {
    f.expires_after_secs.is_none() && f.coverage_note.is_none()
}

impl Evidence {
    #[allow(clippy::too_many_arguments)]
    pub fn known(
        kind: FactKind,
        subtype: FactSubtype,
        value: FactValue,
        source: EvidenceSource,
        observed_at: u64,
    ) -> Self {
        Self {
            kind,
            subtype,
            status: FactStatus::Known(value),
            source,
            observed_at,
            event_at: None,
            freshness: Freshness::none(),
            note: None,
        }
    }

    pub fn unknown(
        kind: FactKind,
        subtype: FactSubtype,
        source: EvidenceSource,
        observed_at: u64,
        reason: impl Into<Reason>,
    ) -> Self {
        Self {
            kind,
            subtype,
            status: FactStatus::Unknown {
                reason: reason.into(),
            },
            source,
            observed_at,
            event_at: None,
            freshness: Freshness::none(),
            note: None,
        }
    }

    pub fn unavailable(
        kind: FactKind,
        subtype: FactSubtype,
        source: EvidenceSource,
        observed_at: u64,
        reason: impl Into<Reason>,
    ) -> Self {
        Self {
            kind,
            subtype,
            status: FactStatus::Unavailable {
                reason: reason.into(),
            },
            source,
            observed_at,
            event_at: None,
            freshness: Freshness::none(),
            note: None,
        }
    }

    pub fn conflicting(
        kind: FactKind,
        subtype: FactSubtype,
        candidates: Vec<FactValue>,
        source: EvidenceSource,
        observed_at: u64,
        reason: impl Into<Reason>,
    ) -> Self {
        Self {
            kind,
            subtype,
            status: FactStatus::Conflicting {
                candidates,
                reason: reason.into(),
            },
            source,
            observed_at,
            event_at: None,
            freshness: Freshness::none(),
            note: None,
        }
    }

    pub fn with_event_at(mut self, event_at: u64) -> Self {
        self.event_at = Some(event_at);
        self
    }

    pub fn with_freshness(mut self, freshness: Freshness) -> Self {
        self.freshness = freshness;
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// True when this fact's stated expiry has passed as of `now` and it
    /// should be rechecked rather than trusted (#55/#61's short-lived
    /// current-use facts; existing action boundaries call this).
    pub fn is_stale(&self, now: u64) -> bool {
        match self.freshness.expires_after_secs {
            Some(secs) => now.saturating_sub(self.observed_at) > secs,
            None => false,
        }
    }

    /// Whether this fact resolved to a known value at all (vs.
    /// unknown/unavailable/conflicting). Never treat `false` here as
    /// "therefore unused/absent" -- see the module docs.
    pub fn is_known(&self) -> bool {
        matches!(self.status, FactStatus::Known(_))
    }
}

/// Filters a unit's evidence list to one [`FactKind`] domain, the shape
/// every presentation layer (#60) reads.
pub fn of_kind(evidence: &[Evidence], kind: FactKind) -> Vec<&Evidence> {
    evidence.iter().filter(|e| e.kind == kind).collect()
}

/// Returns evidence that is stale as of `now` (#55/#61 rechecks these at
/// action boundaries instead of trusting them indefinitely).
pub fn stale(evidence: &[Evidence], now: u64) -> Vec<&Evidence> {
    evidence.iter().filter(|e| e.is_stale(now)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_known() -> Evidence {
        Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            FactValue::Timestamp(1_700_000_000),
            EvidenceSource::FilesystemMetadata {
                detail: "newest recorded modification among measured children".into(),
            },
            1_700_100_000,
        )
        .with_event_at(1_700_000_000)
        .with_freshness(Freshness::with_coverage(
            "only measured children the folded walk recorded",
        ))
    }

    #[test]
    fn round_trips_through_json() {
        let ev = sample_known();
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: Evidence = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
        // Provenance/freshness are not optional decoration: assert the
        // actual fields survive the round trip, not just equality of the
        // whole struct (which could pass with two independently-wrong
        // copies of a buggy Default impl).
        assert!(matches!(
            back.source,
            EvidenceSource::FilesystemMetadata { .. }
        ));
        assert_eq!(
            back.freshness.coverage_note.as_deref(),
            Some("only measured children the folded walk recorded")
        );
        assert_eq!(back.event_at, Some(1_700_000_000));
    }

    #[test]
    fn unknown_is_not_serialized_as_a_known_false_or_zero() {
        let ev = Evidence::unknown(
            FactKind::Consumer,
            FactSubtype::DeclaredConsumer,
            EvidenceSource::ConfigDeclaration {
                path: "/proj/.tool-versions".into(),
            },
            10,
            "no reference found in scanned config",
        );
        let json = serde_json::to_value(&ev).unwrap();
        // The tempting shortcut this test rejects: collapsing "unknown"
        // into `FactValue::Bool(false)` or omitting the field, either of
        // which a renderer could mistake for "no consumer" (verdict).
        assert_eq!(json["status"]["status"], "unknown");
        assert!(json["status"].get("value").is_none());
        let back: Evidence = serde_json::from_value(json).unwrap();
        assert!(!back.is_known());
        assert!(matches!(back.status, FactStatus::Unknown { .. }));
    }

    #[test]
    fn unavailable_is_distinct_from_unknown() {
        let unavailable = Evidence::unavailable(
            FactKind::CurrentUse,
            FactSubtype::Process,
            EvidenceSource::ProcessQuery {
                tool: "lsof".into(),
            },
            10,
            "permission denied querying open files",
        );
        let unknown = Evidence::unknown(
            FactKind::CurrentUse,
            FactSubtype::Process,
            EvidenceSource::ProcessQuery {
                tool: "lsof".into(),
            },
            10,
            "no matching open file handle found",
        );
        // The tempting shortcut: treating "the query failed" the same as
        // "the query succeeded and found nothing" -- they mean very
        // different things ("no consumer needs it" vs "we don't know").
        assert_ne!(unavailable.status, unknown.status);
        match (&unavailable.status, &unknown.status) {
            (FactStatus::Unavailable { .. }, FactStatus::Unknown { .. }) => {}
            _ => panic!("expected distinct Unavailable/Unknown variants"),
        }
    }

    #[test]
    fn conflicting_keeps_every_candidate_rather_than_picking_one() {
        let ev = Evidence::conflicting(
            FactKind::Recovery,
            FactSubtype::Rebuild,
            vec![
                FactValue::Text("network_fetch".into()),
                FactValue::Text("local_rebuild".into()),
            ],
            EvidenceSource::Inferred {
                basis: "two lockfiles disagree on origin".into(),
            },
            10,
            "Cargo.lock and vendor manifest disagree on source origin",
        );
        match &ev.status {
            FactStatus::Conflicting { candidates, .. } => assert_eq!(candidates.len(), 2),
            other => panic!("expected Conflicting, got {other:?}"),
        }
    }

    #[test]
    fn stale_after_expiry_recheck_window() {
        let ev = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Process,
            FactValue::Bool(true),
            EvidenceSource::ProcessQuery {
                tool: "lsof".into(),
            },
            1000,
        )
        .with_freshness(Freshness::expires_after(30));
        assert!(!ev.is_stale(1010));
        assert!(!ev.is_stale(1030));
        assert!(ev.is_stale(1031));
    }

    #[test]
    fn no_expiry_is_never_stale() {
        let ev = Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            FactValue::Timestamp(1),
            EvidenceSource::FilesystemMetadata {
                detail: "mtime".into(),
            },
            1,
        );
        assert!(!ev.is_stale(u64::MAX));
    }

    #[test]
    fn of_kind_and_stale_filters_are_independent() {
        let list = vec![
            sample_known(),
            Evidence::known(
                FactKind::CurrentUse,
                FactSubtype::Process,
                FactValue::Bool(true),
                EvidenceSource::ProcessQuery {
                    tool: "lsof".into(),
                },
                1000,
            )
            .with_freshness(Freshness::expires_after(1)),
        ];
        assert_eq!(of_kind(&list, FactKind::Activity).len(), 1);
        assert_eq!(of_kind(&list, FactKind::CurrentUse).len(), 1);
        assert_eq!(stale(&list, 1005).len(), 1);
        assert_eq!(stale(&list, 1000).len(), 0);
    }

    #[test]
    fn empty_evidence_list_skips_serialization() {
        // Contract requirement: `evidence: Vec<Evidence>` fields on
        // artifact/external/agent/nested rows must vanish from JSON when
        // empty, not print `"evidence": []` on every single row.
        #[derive(Serialize)]
        struct Row {
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            evidence: Vec<Evidence>,
        }
        let row = Row { evidence: vec![] };
        let json = serde_json::to_string(&row).unwrap();
        assert_eq!(json, "{}");
    }
}
