//! #101 acceptance, mirroring `agent_units_actions_new_adapters.rs`'s
//! coverage for the nine remaining named tools #96/#97/#98/#99 added:
//! Gemini CLI, Pi, Aider, GitHub Copilot CLI, Cursor, Windsurf, Cline,
//! Roo Code and Continue. Disposable fixtures only -- no real tool home
//! is read or written by these tests (PRIVACY IS A HARD RULE); every
//! fixture session body carries a canary string, and every assertion
//! that inspects serialized output checks that canary never appears in
//! it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::actions;
use swamp_core::agents::{AgentActionCapability, ProjectLinkState, discover_and_measure};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

const CANARY: &str = "CANARY-REMAINING-TOOLS-DO-NOT-LEAK-4f9c";

#[test]
fn copilot_exact_local_session_is_removable_without_guessing_a_project() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".copilot");
    write(&home.join("settings.json"), b"{}");
    write(
        &home.join("session-state/selected/events.jsonl"),
        CANARY.as_bytes(),
    );
    write(
        &home.join("session-state/selected/checkpoints/snapshot"),
        b"unique checkpoint",
    );
    write(
        &home.join("session-state/retained/events.jsonl"),
        b"retained session",
    );
    write(
        &home.join("session-state/unrecognized/file"),
        b"not a documented session",
    );
    write(&home.join("session-store.db"), b"protected index");
    let store = tempfile::tempdir().unwrap();
    let units = units_for(
        tmp.path(),
        HashMap::new(),
        "github-copilot-cli",
        &[],
        store.path(),
    );
    let path = home.join("session-state/selected");
    let unit = units.iter().find(|u| u.path == path).unwrap();
    assert!(matches!(
        unit.project_link,
        ProjectLinkState::Unresolved { .. }
    ));
    let plan = actions::propose_agents(&units, std::slice::from_ref(&path), "test").unwrap();
    assert!(
        plan[0]
            .warnings()
            .iter()
            .any(|s| s.contains("resume/rewind"))
    );
    assert_refused(&units, &home.join("session-state/unrecognized"));
    assert_eq!(execute_one(&units, &path, store.path()), "completed");
    assert!(!path.exists());
    assert!(home.join("session-state/retained/events.jsonl").exists());
    assert!(home.join("session-store.db").exists());
    no_canary_anywhere(&units, store.path());
}

fn write(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const ALL_TOOL_DETECTORS: &[&str] = &[
    "claude-code",
    "codex",
    "codex-desktop",
    "oh-my-pi",
    "opencode",
    "gemini-cli",
    "pi",
    "aider",
    "github-copilot-cli",
    "cursor",
    "windsurf",
    "cline",
    "roo-code",
    "continue",
];

fn only_detector(id: &str) -> ScanConfig {
    // An unknown id would authorize nothing and leave the fixture
    // passing vacuously, so the allow-list entry is checked against the
    // catalog this file enumerates.
    assert!(
        ALL_TOOL_DETECTORS.contains(&id),
        "{id:?} is not one of this file's tool detector ids"
    );
    ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        // Allow-list of exactly the tool under test. The previous
        // deny-list named every *other* tool detector and left the rest
        // of the catalog running, including core_simulator, whose
        // absolute system path ignores the fixture home.
        disabled_detectors: Vec::new(),
        enabled_detectors: vec![id.to_string()],
    }
}

fn units_for(
    home: &Path,
    env_vars: HashMap<String, String>,
    detector_id: &str,
    project_worktrees: &[PathBuf],
    store: &Path,
) -> Vec<swamp_core::agents::AgentUnit> {
    let env = Environment::fixture(home.to_path_buf(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(&env, &only_detector(detector_id), &[], &registry, 1);
    discover_and_measure(
        &scope,
        project_worktrees,
        Some(store),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap()
}

fn execute_one(units: &[swamp_core::agents::AgentUnit], path: &Path, store: &Path) -> String {
    let plan =
        actions::propose_agents(units, std::slice::from_ref(&path.to_path_buf()), "test").unwrap();
    let unit = &plan[0];
    let meta = unit.agent_meta().unwrap();
    let trash = store.join("trash");
    fs::create_dir_all(&trash).unwrap();
    let at = swamp_core::entities::now();
    let result = match &meta.session_members {
        Some(members) => actions::trash_agent_session(meta, unit.path(), members, &trash, at),
        None => actions::trash_agent_cache(unit.path(), &trash, at),
    };
    match result {
        Ok(_) => "completed".to_string(),
        Err(_) => "failed".to_string(),
    }
}

fn assert_refused(units: &[swamp_core::agents::AgentUnit], path: &Path) {
    let err = actions::propose_agents(units, std::slice::from_ref(&path.to_path_buf()), "test")
        .unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("protected")
            || format!("{err}")
                .to_lowercase()
                .contains("swamp has no trash move"),
        "{err}"
    );
}

fn no_canary_anywhere(units: &[swamp_core::agents::AgentUnit], store: &Path) {
    let serialized = serde_json::to_string(units).unwrap();
    assert!(
        !serialized.contains(CANARY),
        "content leaked into AgentUnit JSON"
    );
    if let Ok(ledger) = swamp_core::ledger::Ledger::open(store.join("ledger.parquet"))
        && let Ok(records) = ledger.all()
    {
        let ledger = format!("{records:?}");
        assert!(!ledger.contains(CANARY), "content leaked into the ledger");
    }
}

// ---------------------------------------------------------------------
// Gemini CLI
// ---------------------------------------------------------------------

#[test]
fn gemini_cli_bin_removal_preserves_settings_and_leaves_project_hash_unresolved() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("gemini-home");
    write(&home.join("settings.json"), b"{}");
    // `tmp/bin`, not `bin`: upstream builds the downloaded-tools cache as
    // `join(getGlobalTempDir(), 'bin')` (`packages/core/src/config/storage.ts:195-201`
    // @ `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`). `~/.gemini/bin` was a
    // path no version writes, so this test used to act on a directory
    // that would never exist on a real machine.
    write(&home.join("tmp/bin/litert/tool"), b"binary-bytes");
    let hash = "a".repeat(64);
    write(
        &home.join(format!("tmp/{hash}/shell_history")),
        format!("ls\n{CANARY}\n").as_bytes(),
    );
    let store = tempfile::tempdir().unwrap();
    let mut env_vars = HashMap::new();
    env_vars.insert("GEMINI_CLI_HOME".to_string(), home.display().to_string());
    let home_dummy = tempfile::tempdir().unwrap();
    let units = units_for(home_dummy.path(), env_vars, "gemini-cli", &[], store.path());

    let shell = units
        .iter()
        .find(|u| u.category == swamp_core::agents::AgentCategory::Logs)
        .expect("shell history identified");
    assert!(matches!(
        shell.project_link,
        ProjectLinkState::Unresolved { .. }
    ));

    let bin_dir = home.join("tmp").join("bin");
    let status = execute_one(&units, &bin_dir, store.path());
    assert_eq!(status, "completed");
    assert!(!bin_dir.exists());
    assert!(
        home.join("settings.json").exists(),
        "protected config survives"
    );
    assert_refused(&units, &home.join("settings.json"));
    no_canary_anywhere(&units, store.path());
    let _ = root;
}

// ---------------------------------------------------------------------
// Pi
// ---------------------------------------------------------------------

#[test]
fn pi_session_removal_preserves_the_repo_and_leaks_no_content() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("pi-agent");
    let repo = root.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    write(&home.join("settings.json"), b"{}");
    let jsonl = home.join("sessions/-repo/1.jsonl");
    write(
        &jsonl,
        format!(
            "{{\"id\":\"1\",\"cwd\":\"{}\"}}\n{{\"id\":\"2\",\"content\":\"{CANARY}\"}}\n",
            repo.display()
        )
        .as_bytes(),
    );
    let store = tempfile::tempdir().unwrap();
    let mut env_vars = HashMap::new();
    env_vars.insert(
        "PI_CODING_AGENT_DIR".to_string(),
        home.display().to_string(),
    );
    let home_dummy = tempfile::tempdir().unwrap();
    let units = units_for(home_dummy.path(), env_vars, "pi", &[], store.path());
    let session = units.iter().find(|u| u.path == jsonl).expect("session");
    assert!(matches!(
        session.project_link,
        ProjectLinkState::Linked { .. }
    ));

    let status = execute_one(&units, &jsonl, store.path());
    assert_eq!(status, "completed");
    assert!(!jsonl.exists());
    assert!(repo.join(".git").exists());
    no_canary_anywhere(&units, store.path());
    let _ = root;
}

// ---------------------------------------------------------------------
// Aider -- project-local, not detector-home units.
// ---------------------------------------------------------------------

#[test]
fn aider_repo_history_removal_preserves_the_tags_cache_and_the_repo() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    write(
        &repo.join(".aider.chat.history.md"),
        format!("# chat\nUser: {CANARY}\n").as_bytes(),
    );
    write(&repo.join(".aider.tags.cache.v3/tags.db"), b"cache-bytes");
    let store = tempfile::tempdir().unwrap();
    let home_dummy = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        HashMap::new(),
        "aider",
        std::slice::from_ref(&repo),
        store.path(),
    );
    let history_path = repo.join(".aider.chat.history.md");
    let history = units
        .iter()
        .find(|u| u.path == history_path)
        .expect("chat history identified");
    assert!(matches!(
        history.project_link,
        ProjectLinkState::Linked { .. }
    ));
    assert_eq!(history.action, AgentActionCapability::SessionRemoval);

    let status = execute_one(&units, &history_path, store.path());
    assert_eq!(status, "completed");
    assert!(!history_path.exists());
    assert!(
        repo.join(".aider.tags.cache.v3").exists(),
        "removing chat history must not touch the regenerable tags cache"
    );
    assert!(repo.join(".git").exists());
    no_canary_anywhere(&units, store.path());
    let _ = root;
}

#[test]
fn aider_disabled_detector_turns_off_both_home_and_repo_units() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo.join(".aider.chat.history.md"), b"# chat");
    let home_dummy = tempfile::tempdir().unwrap();
    let env = Environment::fixture(
        home_dummy.path().to_path_buf(),
        HashMap::new(),
        Platform::MacOS,
    );
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        // Deliberately both: `aider` is the only detector this scope
        // would consider, and it is explicitly disabled -- which is the
        // whole point of this test. Bounding the allow-list as well
        // keeps a fixture from reaching any other detector's absolute
        // system paths.
        disabled_detectors: vec!["aider".to_string()],
        enabled_detectors: vec!["aider".to_string()],
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1);
    let store = tempfile::tempdir().unwrap();
    let units = discover_and_measure(
        &scope,
        std::slice::from_ref(&repo),
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(
        units.iter().all(|u| u.tool_id != "aider"),
        "disabling the aider detector must also turn off its per-repo units"
    );
}

// ---------------------------------------------------------------------
// GitHub Copilot CLI
// ---------------------------------------------------------------------

#[test]
fn copilot_cli_command_history_removal_preserves_config_and_session_store() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("copilot-home");
    write(&home.join("settings.json"), b"{}");
    write(&home.join("session-store.db"), b"sqlite-bytes");
    write(
        &home.join("command-history-state/recall.log"),
        format!("cmd1\n{CANARY}\n").as_bytes(),
    );
    let store = tempfile::tempdir().unwrap();
    let mut env_vars = HashMap::new();
    env_vars.insert("COPILOT_HOME".to_string(), home.display().to_string());
    let home_dummy = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        env_vars,
        "github-copilot-cli",
        &[],
        store.path(),
    );

    let cmd_hist = home.join("command-history-state");
    let status = execute_one(&units, &cmd_hist, store.path());
    assert_eq!(status, "completed");
    assert!(!cmd_hist.exists());
    assert!(home.join("session-store.db").exists());
    assert!(home.join("settings.json").exists());
    assert_refused(&units, &home.join("session-store.db"));
    no_canary_anywhere(&units, store.path());
    let _ = root;
}

// ---------------------------------------------------------------------
// Cursor / Windsurf -- editor-profile shape.
// ---------------------------------------------------------------------

#[test]
fn cursor_global_database_is_never_a_supported_action() {
    let home_dummy = tempfile::tempdir().unwrap();
    let vscdb = home_dummy
        .path()
        .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    write(&vscdb, b"sqlite-bytes");
    let store = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        HashMap::new(),
        "cursor",
        &[],
        store.path(),
    );
    let db = units.iter().find(|u| u.path == vscdb).expect("global db");
    assert!(db.protected);
    assert_eq!(db.action, AgentActionCapability::None);
    assert_refused(&units, &vscdb);
}

#[test]
fn windsurf_unrecognized_layout_is_an_explicit_residual_not_a_guess() {
    let home_dummy = tempfile::tempdir().unwrap();
    write(
        &home_dummy
            .path()
            .join("Library/Application Support/Windsurf/unrelated.txt"),
        b"hello",
    );
    let store = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        HashMap::new(),
        "windsurf",
        &[],
        store.path(),
    );
    assert!(
        units
            .iter()
            .any(|u| u.relative_path == "(unsupported layout version)"),
        "{units:?}"
    );
}

// ---------------------------------------------------------------------
// Cline / Roo Code -- multi-host, never merged.
// ---------------------------------------------------------------------

#[test]
fn cline_two_hosts_with_the_same_task_id_stay_independent() {
    let home_dummy = tempfile::tempdir().unwrap();
    let vscode_task = home_dummy.path().join(
        "Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/tasks/t1/\
         api_conversation_history.json",
    );
    let cursor_task = home_dummy.path().join(
        "Library/Application Support/Cursor/User/globalStorage/saoudrizwan.claude-dev/tasks/t1/\
         api_conversation_history.json",
    );
    write(
        &vscode_task,
        format!("[{{\"content\":\"{CANARY}\"}}]").as_bytes(),
    );
    write(&cursor_task, b"[]");
    let store = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        HashMap::new(),
        "cline",
        &[],
        store.path(),
    );
    let sessions: Vec<_> = units
        .iter()
        .filter(|u| u.category == swamp_core::agents::AgentCategory::Sessions)
        .collect();
    assert_eq!(sessions.len(), 2, "{units:?}");
    assert!(
        sessions
            .iter()
            .any(|u| u.relative_path.starts_with("VS Code/"))
    );
    assert!(
        sessions
            .iter()
            .any(|u| u.relative_path.starts_with("Cursor/"))
    );
    // Removing the VS Code host's task must not touch Cursor's.
    let vscode_dir = vscode_task.parent().unwrap();
    let status = execute_one(&units, vscode_dir, store.path());
    assert_eq!(status, "completed");
    assert!(!vscode_dir.exists());
    assert!(cursor_task.exists(), "the other host's task is untouched");
    no_canary_anywhere(&units, store.path());
}

#[test]
fn roo_code_remote_host_task_is_identified_and_removable() {
    let home_dummy = tempfile::tempdir().unwrap();
    let task_dir = home_dummy
        .path()
        .join(".vscode-server/data/User/globalStorage/rooveterinaryinc.roo-cline/tasks/t1");
    write(&task_dir.join("ui_messages.json"), b"[]");
    let store = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        HashMap::new(),
        "roo-code",
        &[],
        store.path(),
    );
    let u = units.iter().find(|u| u.path == task_dir).expect("task");
    assert!(
        u.relative_path
            .starts_with("VS Code Server (remote)/tasks/")
    );
    let status = execute_one(&units, &task_dir, store.path());
    assert_eq!(status, "completed");
    assert!(!task_dir.exists());
}

// ---------------------------------------------------------------------
// Continue
// ---------------------------------------------------------------------

#[test]
fn continue_session_removal_preserves_the_index_and_config() {
    let home_dummy = tempfile::tempdir().unwrap();
    write(
        &home_dummy.path().join(".continue/config.yaml"),
        b"models: []",
    );
    write(
        &home_dummy.path().join(".continue/sessions/sessions.json"),
        b"[{\"id\":\"s1\"}]",
    );
    let session = home_dummy.path().join(".continue/sessions/s1.json");
    write(&session, format!("{{\"content\":\"{CANARY}\"}}").as_bytes());
    let store = tempfile::tempdir().unwrap();
    let units = units_for(
        home_dummy.path(),
        HashMap::new(),
        "continue",
        &[],
        store.path(),
    );

    let status = execute_one(&units, &session, store.path());
    assert_eq!(status, "completed");
    assert!(!session.exists());
    assert!(
        home_dummy
            .path()
            .join(".continue/sessions/sessions.json")
            .exists()
    );
    assert!(home_dummy.path().join(".continue/config.yaml").exists());
    assert_refused(
        &units,
        &home_dummy.path().join(".continue/sessions/sessions.json"),
    );
    no_canary_anywhere(&units, store.path());
}
