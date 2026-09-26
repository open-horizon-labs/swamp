// repo: github.com/openai/codex
// commit: ac7634b9f73ec1bf96466be7a5869f0949d20b30 (2026-09-22T11:44:00Z)  path: codex-rs/rollout/src/list.rs
// retrieved: 2026-09-22
// --- line 367: sessions root ---
    let root = codex_home.join(SESSIONS_SUBDIR);
// --- lines 1058-1078: year/month/day dir walk ---
    visitor: &mut impl RolloutFileVisitor,
) -> io::Result<()> {
    let year_dirs = collect_dirs_desc(root, |s| s.parse::<u16>().ok()).await?;

    'outer: for (_year, year_path) in year_dirs.iter() {
        if *scanned_files >= MAX_SCAN_FILES {
            break;
        }
        let month_dirs = collect_dirs_desc(year_path, |s| s.parse::<u8>().ok()).await?;
        for (_month, month_path) in month_dirs.iter() {
            if *scanned_files >= MAX_SCAN_FILES {
                break 'outer;
            }
            let day_dirs = collect_dirs_desc(month_path, |s| s.parse::<u8>().ok()).await?;
            for (_day, day_path) in day_dirs.iter() {
                if *scanned_files >= MAX_SCAN_FILES {
                    break 'outer;
                }
                let day_files = collect_rollout_day_files(day_path).await?;
                for (ts, id, path) in day_files.into_iter() {
                    *scanned_files += 1;
// --- lines 1689-1697: rollout_date_parts (YYYY/MM/DD) ---
/// Extract the `YYYY/MM/DD` directory components from a rollout filename.
pub fn rollout_date_parts(file_name: &OsStr) -> Option<(String, String, String)> {
    let name = file_name.to_string_lossy();
    let date = name.strip_prefix("rollout-")?.get(..10)?;
    let year = date.get(..4)?.to_string();
    let month = date.get(5..7)?.to_string();
    let day = date.get(8..10)?.to_string();
    Some((year, month, day))
}
// --- lines 1676-1680: sessions + archived_sessions ---
    for subdir in [SESSIONS_SUBDIR, ARCHIVED_SESSIONS_SUBDIR] {
        let path = find_rollout_path_by_rollout_id_from_filenames(
            codex_home.join(subdir).as_path(),
            rollout_id,
        )
