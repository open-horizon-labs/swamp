// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/cli/src/doctor/desktop.rs
// retrieved: 2026-09-22
// --- lines 135-150: desktop_log_root (macos + windows only; Linux -> None) ---
fn desktop_log_root(identity: &str) -> Option<PathBuf> {
    let root = match env::consts::OS {
        "macos" => PathBuf::from(env::var_os("HOME")?)
            .join("Library/Logs")
            .join(identity),
        "windows" => {
            let local = env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(|| {
                env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData/Local"))
            })?;
            local.join("Codex/Logs")
        }
        _ => return None,
    };

    Some(root)
}
