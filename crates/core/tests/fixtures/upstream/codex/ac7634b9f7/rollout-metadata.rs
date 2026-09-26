// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/rollout/src/metadata.rs
// retrieved: 2026-09-22
// --- lines 57-60: cwd from SessionMeta ---
    builder.agent_role = session_meta.meta.agent_role.clone();
    builder.agent_path = session_meta.meta.agent_path.clone();
    builder.cwd = session_meta.meta.cwd.clone();
    builder.cli_version = Some(session_meta.meta.cli_version.clone());
// --- lines 262-265: sessions/archived roots ---

    let sessions_root = codex_home.join(SESSIONS_SUBDIR);
    let archived_root = codex_home.join(ARCHIVED_SESSIONS_SUBDIR);
    let mut rollout_paths: Vec<BackfillRolloutPath> = Vec::new();
