// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/protocol/src/protocol.rs
// retrieved: 2026-09-22
// --- lines 3099-3101, 3117-3119: SessionMeta.cwd ---
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema, TS)]
pub struct SessionMeta {
    /// ChatGPT user that created this thread; absent when unavailable or for older threads.
// ...
    pub parent_thread_id: Option<ThreadId>,
    pub timestamp: String,
    pub cwd: PathBuf,
