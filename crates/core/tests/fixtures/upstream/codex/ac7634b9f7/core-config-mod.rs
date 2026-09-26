// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/core/src/config/mod.rs
// retrieved: 2026-09-22
// --- lines 4003-4007: log_dir default = CODEX_HOME/log ---
        let log_dir = cfg
            .log_dir
            .as_ref()
            .map(AbsolutePathBuf::to_path_buf)
            .unwrap_or_else(|| codex_home.join("log").to_path_buf());
// --- lines 266-270: SQLITE_HOME_ENV read ---

fn resolve_sqlite_home_env(resolved_cwd: &Path) -> Option<AbsolutePathBuf> {
    let raw = std::env::var(codex_state::SQLITE_HOME_ENV).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
