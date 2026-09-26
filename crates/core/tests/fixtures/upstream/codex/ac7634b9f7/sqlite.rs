// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/state/src/sqlite.rs
// retrieved: 2026-09-22
// --- lines 29-34: named filename consts ---
const LOGS_DB_FILENAME: &str = "logs_2.sqlite";
const GOALS_DB_FILENAME: &str = "goals_1.sqlite";
const MEMORIES_DB_FILENAME: &str = "memories_1.sqlite";
const QUEUE_DB_FILENAME: &str = "queue_1.sqlite";
const STATE_DB_FILENAME: &str = "state_5.sqlite";
const THREAD_HISTORY_DB_FILENAME: &str = "thread_history_1.sqlite";
// --- lines 83-87: 7th DB, filename inline ---
const MEMORIES_V2_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "memories v2 DB",
    filename: "memories_v2_1.sqlite",
    ..MEMORIES_DB
};
// --- lines 105-113: RUNTIME_DBS ---
const RUNTIME_DBS: [RuntimeDbSpec; 7] = [
    STATE_DB,
    LOGS_DB,
    GOALS_DB,
    MEMORIES_DB,
    MEMORIES_V2_DB,
    QUEUE_DB,
    THREAD_HISTORY_DB,
];
// --- lines 45-49: path = codex_home.join(filename) ---
impl RuntimeDbSpec {
    fn path(self, codex_home: &Path) -> PathBuf {
        codex_home.join(self.filename)
    }
}
