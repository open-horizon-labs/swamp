use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn label(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// The inverse of [`Self::label`]. Exhaustive by construction (R18a
    /// nested-artifact typed columns): an unrecognized label falls back
    /// to [`Self::Low`], the least trusting reading, never a panic.
    pub fn from_label(label: &str) -> Self {
        match label {
            "high" => Self::High,
            "medium" => Self::Medium,
            _ => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecoveryContract {
    LocalRebuild,
    NetworkFetch,
    Irrecoverable,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactKind {
    BuildOutput,
    DependencyTree,
    Git,
    DockerImage,
    DockerCache,
    DockerVolume,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactMeta {
    pub observed_at: u64,
    pub source: String,
    pub confidence: Confidence,
    pub horizon_exceeded: bool,
}

impl FactMeta {
    pub fn now(source: impl Into<String>, confidence: Confidence) -> Self {
        Self {
            observed_at: now(),
            source: source.into(),
            confidence,
            horizon_exceeded: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub path: PathBuf,
    pub confidence: Confidence,
    pub meta: FactMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub project_id: Option<String>,
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub relative_path: Option<PathBuf>,
    pub bytes: u64,
    pub recovery: RecoveryContract,
    pub present: bool,
    pub regrowth_count: u32,
    pub meta: FactMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub repo_id: String,
    pub linked: bool,
    pub locked: bool,
    pub meta: FactMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySignal {
    pub project_id: String,
    pub name: String,
    pub value: String,
    pub meta: FactMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub volume_id: u64,
    pub roots: Vec<PathBuf>,
    pub artifacts: Vec<Artifact>,
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub signals: Vec<ActivitySignal>,
    pub observed_at: u64,
    pub source: String,
    pub coverage_bytes: u64,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn id_for(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}
