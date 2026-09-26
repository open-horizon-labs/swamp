// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/config/src/config_toml.rs
// retrieved: 2026-09-22
// --- lines 367-374: sqlite_home + log_dir config keys ---
    /// Directory where Codex stores the SQLite state DB.
    /// Defaults to `$CODEX_SQLITE_HOME` when set. Otherwise uses `$CODEX_HOME`.
    pub sqlite_home: Option<AbsolutePathBuf>,

    /// Directory where Codex writes log files. Setting this value explicitly
    /// also enables the TUI text log in this directory.
    /// Defaults to `$CODEX_HOME/log`.
    pub log_dir: Option<AbsolutePathBuf>,
