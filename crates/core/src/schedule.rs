//! Opt-in scheduled observation: a per-user LaunchAgent that runs
//! `swamp observe <roots>` on a fixed interval.
//!
//! Ported from the mole `integrate` branch's `lib/manage/schedule.sh` +
//! `docs/scheduled-inventory-refresh.md` design: a scheduled job, not a
//! daemon. `launchd` starts one process, it exits. No resident process, no
//! menu bar, no notifications. Off by default, installed only by the
//! explicit `swamp schedule --every <interval>` command.
//!
//! The scheduled program is `swamp observe`, which only walks and
//! writes the growth store -- it never renders a report and has no path to
//! any destructive command (there are none in this tool).

use crate::fs_gate::{self, read::read_owned_string, store};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub const LABEL: &str = "com.open-horizon-labs.swamp.observe";

/// Default watchdog budget for one `observe` invocation, in seconds.
pub const DEFAULT_OBSERVE_TIMEOUT_SECS: u64 = 1800;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

/// Swamp's LaunchAgent plist, where the gate resolves it
/// (`fs_gate::store::launch_agent_plist`).
pub fn plist_path() -> PathBuf {
    store::launch_agent_plist().unwrap_or_else(|_| PathBuf::from(format!("{LABEL}.plist")))
}

/// Where a scheduled `observe` writes its log.
///
/// `~/Library/Logs/swamp` on macOS, where Console.app looks. On Linux
/// the XDG base directory spec's *state* directory --
/// `$XDG_STATE_HOME/swamp`, default `~/.local/state/swamp` -- which is
/// what that spec's state category is for ("logs, history, recently
/// used files"), rather than data (`$XDG_DATA_HOME`, where the growth
/// store lives) or cache. A relative `$XDG_STATE_HOME` is ignored, as
/// the spec requires. `SWAMP_LOG_DIR` still overrides both.
pub fn log_dir() -> PathBuf {
    match std::env::var_os("SWAMP_LOG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let dir = default_log_dir();
            store::refuse_real_user_default("observe log directory", &dir, store::Resolve::Display);
            dir
        }
    }
}

fn default_log_dir() -> PathBuf {
    match crate::platform::Os::current() {
        crate::platform::Os::MacOs => home().join("Library/Logs/swamp"),
        crate::platform::Os::Linux => xdg_dir("XDG_STATE_HOME", ".local/state").join("swamp"),
    }
}

/// An XDG base directory, honouring the spec's rule that a relative
/// value "should be considered invalid and ignored".
fn xdg_dir(var: &str, fallback_rel: &str) -> PathBuf {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => home().join(fallback_rel),
    }
}

/// Whether this build can install an unattended periodic observation,
/// and -- when it cannot -- the sentence that says so.
///
/// One answer shared by `install`, `uninstall` and `status`. A platform
/// with no scheduler must refuse: writing a LaunchAgent plist into a
/// `~/Library/LaunchAgents` no daemon reads would report success for a
/// job that will never run, which is worse than having no scheduler.
pub fn scheduling() -> crate::platform::Scheduling {
    crate::platform::Scheduling::for_os(crate::platform::Os::current())
}

pub fn log_file() -> PathBuf {
    log_dir().join("observe.log")
}

/// Parses a human interval (`"30m"`, `"1h"`, `"12h"`, `"1d"`, or a bare
/// number of seconds) into seconds. Rejects anything unparseable rather
/// than silently defaulting to some other cadence.
pub fn parse_interval(raw: &str) -> Result<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("missing interval; use --every 30m, 1h, 12h or 1d");
    }
    let seconds = crate::growth::parse_duration_secs(raw)
        .with_context(|| format!("invalid interval: {raw} (use a number with s, m, h or d)"))?;
    if seconds == 0 {
        bail!("interval must be greater than zero: {raw}");
    }
    Ok(seconds)
}

/// Renders seconds back into the shortest exact human form.
pub fn format_interval(seconds: u64) -> String {
    if seconds.is_multiple_of(86400) {
        format!("{}d", seconds / 86400)
    } else if seconds.is_multiple_of(3600) {
        format!("{}h", seconds / 3600)
    } else if seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(target_os = "macos")]
fn test_mode() -> bool {
    std::env::var("SWAMP_TEST_MODE").is_ok_and(|v| v == "1")
}

/// `launchctl` indirection. Under `SWAMP_TEST_MODE=1` every call is a
/// no-op that prints what it would have run, so no test suite can ever
/// register a real job on the machine running it. Compiled on macOS
/// only: a Linux build contains no path that could run `launchctl`.
#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> Result<bool> {
    if test_mode() {
        println!("[test-mode] launchctl {}", args.join(" "));
        return Ok(true);
    }
    let out = fs_gate::spawn::run(
        fs_gate::spawn::Program::Launchctl,
        args,
        std::time::Duration::from_secs(30),
    )
    .context("spawn launchctl")?;
    Ok(out.success())
}

#[cfg(target_os = "macos")]
fn domain() -> String {
    let uid = std::env::var("SWAMP_UID").ok().unwrap_or_else(|| {
        fs_gate::spawn::run(
            fs_gate::spawn::Program::Id,
            ["-u"],
            std::time::Duration::from_secs(5),
        )
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "0".to_string())
    });
    format!("gui/{uid}")
}

#[cfg(target_os = "macos")]
fn load_plist(path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    if run_launchctl(&["bootstrap", &domain(), &path_str])? {
        return Ok(());
    }
    // Fallback to the older launchctl surface.
    run_launchctl(&["load", "-w", &path_str])?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn unload_plist(path: &Path) {
    let path_str = path.to_string_lossy().to_string();
    let service = format!("{}/{LABEL}", domain());
    let _ = run_launchctl(&["bootout", &service]);
    let _ = run_launchctl(&["unload", "-w", &path_str]);
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders the LaunchAgent plist body.
pub fn render_plist(
    exe: &Path,
    roots: &[PathBuf],
    seconds: u64,
    log_path: &Path,
    swamp_dir_env: Option<&str>,
) -> String {
    let mut args = String::new();
    args.push_str(&format!(
        "        <string>{}</string>\n",
        xml_escape(&exe.to_string_lossy())
    ));
    args.push_str("        <string>observe</string>\n");
    for root in roots {
        args.push_str(&format!(
            "        <string>{}</string>\n",
            xml_escape(&root.to_string_lossy())
        ));
    }

    let env_block = match swamp_dir_env {
        Some(dir) => format!(
            "    <key>EnvironmentVariables</key>\n    <dict>\n        <key>SWAMP_DIR</key>\n        <string>{}</string>\n    </dict>\n",
            xml_escape(dir)
        ),
        None => String::new(),
    };

    let log_str = xml_escape(&log_path.to_string_lossy());

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>Label</key>\n\
    <string>{LABEL}</string>\n\
    <key>ProgramArguments</key>\n\
    <array>\n\
{args}\
    </array>\n\
    <key>StartInterval</key>\n\
    <integer>{seconds}</integer>\n\
    <key>RunAtLoad</key>\n\
    <false/>\n\
    <key>ProcessType</key>\n\
    <string>Background</string>\n\
    <key>LowPriorityIO</key>\n\
    <true/>\n\
    <key>Nice</key>\n\
    <integer>10</integer>\n\
{env_block}\
    <key>StandardOutPath</key>\n\
    <string>{log_str}</string>\n\
    <key>StandardErrorPath</key>\n\
    <string>{log_str}</string>\n\
</dict>\n\
</plist>\n"
    )
}

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("resolve current executable")
}

/// `swamp schedule --every <interval> [--collector] <root>...`. `roots`
/// empty (#42/#50) installs `observe` with no positional roots at all:
/// every scheduled fire re-resolves the configured scope fresh (see
/// `Command::Observe`'s `roots.is_empty()` path), rather than replaying
/// whatever roots were present at install time. Passing explicit roots
/// still freezes exactly those, same as before -- an explicit root list
/// has always replaced the configured scope for one invocation, and that
/// includes a scheduled one.
///
/// Which backend installs it is this build's [`scheduling`] answer, and
/// nothing else: a LaunchAgent on macOS, `systemd --user` units on
/// Linux. `collector` asks for the Linux live-watch collector as well;
/// macOS refuses it, because FSEvents already keeps the history a
/// collector would.
pub fn install(interval_raw: &str, roots: &[PathBuf], collector: bool) -> Result<String> {
    match scheduling() {
        // Before anything is parsed, created or written: a platform with
        // no scheduling backend refuses and leaves no state behind.
        crate::platform::Scheduling::Unavailable { .. } => {
            let refusal = scheduling().refusal().unwrap_or_default();
            bail!("{refusal}");
        }
        crate::platform::Scheduling::LaunchdUserAgent => {
            if collector {
                bail!(
                    "--collector is for platforms without persisted change history; macOS has                      one (FSEvents), so a scheduled observation replays it and needs no resident                      process. Nothing has been installed."
                );
            }
            install_launchd(interval_raw, roots)
        }
        crate::platform::Scheduling::SystemdUser => install_systemd(interval_raw, roots, collector),
    }
}

#[cfg(target_os = "macos")]
fn install_launchd(interval_raw: &str, roots: &[PathBuf]) -> Result<String> {
    let seconds = parse_interval(interval_raw)?;
    let exe = current_exe()?;
    let plist = plist_path();
    let log = log_file();

    // Installing over an existing agent replaces it: unload first so
    // launchd never holds two generations of the same label.
    if fs_gate::exists(&plist) {
        unload_plist(&plist);
    }

    let swamp_dir_env = std::env::var("SWAMP_DIR").ok();
    let body = render_plist(&exe, roots, seconds, &log, swamp_dir_env.as_deref());
    store::write_text(store::TextFile::LaunchAgent, &body)
        .with_context(|| format!("write {}", plist.display()))?;

    load_plist(&plist)?;

    let roots_str = if roots.is_empty() {
        "(configured scope, resolved fresh on every run)".to_string()
    } else {
        roots
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    };
    Ok(format!(
        "Scheduled observation every {}\n  Label: {LABEL}\n  Plist: {}\n  Log:   {}\n  Roots: {}\n  Turn it off with: swamp schedule --off\n",
        format_interval(seconds),
        plist.display(),
        log.display(),
        roots_str
    ))
}

#[cfg(not(target_os = "macos"))]
fn install_launchd(_interval_raw: &str, _roots: &[PathBuf]) -> Result<String> {
    bail!("the launchd backend is not compiled into this build; nothing has been installed")
}

#[cfg(target_os = "linux")]
fn install_systemd(interval_raw: &str, roots: &[PathBuf], collector: bool) -> Result<String> {
    let seconds = parse_interval(interval_raw)?;
    let cfg = crate::systemd_user::Config {
        exe: current_exe()?,
        interval_secs: seconds,
        roots: roots.to_vec(),
        swamp_dir: std::env::var("SWAMP_DIR").ok(),
        collector,
    };
    crate::systemd_user::install(
        &mut crate::systemd_user::RealSystemctl,
        &crate::systemd_user::unit_dir()?,
        &cfg,
    )
}

#[cfg(not(target_os = "linux"))]
fn install_systemd(_interval_raw: &str, _roots: &[PathBuf], _collector: bool) -> Result<String> {
    bail!("the systemd backend is not compiled into this build; nothing has been installed")
}

/// `swamp schedule --off`.
pub fn uninstall() -> Result<String> {
    match scheduling() {
        // Nothing could have been installed, so there is nothing to
        // remove and no scheduler to call.
        crate::platform::Scheduling::Unavailable { reason, planned } => Ok(format!(
            "Scheduled observation is not available on this platform: {reason}.\n  Planned in {planned}.\n  Nothing was installed, so nothing was removed.\n"
        )),
        crate::platform::Scheduling::LaunchdUserAgent => uninstall_launchd(),
        crate::platform::Scheduling::SystemdUser => uninstall_systemd(),
    }
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<String> {
    let plist = plist_path();
    if !fs_gate::exists(&plist) {
        unload_plist(&plist);
        return Ok("No scheduled observation is installed\n".to_string());
    }
    unload_plist(&plist);
    store::remove_text(store::TextFile::LaunchAgent)
        .with_context(|| format!("remove {}", plist.display()))?;
    Ok(format!("Removed the scheduled observation ({LABEL})\n"))
}

#[cfg(not(target_os = "macos"))]
fn uninstall_launchd() -> Result<String> {
    bail!("the launchd backend is not compiled into this build")
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> Result<String> {
    crate::systemd_user::uninstall(
        &mut crate::systemd_user::RealSystemctl,
        &crate::systemd_user::unit_dir()?,
    )
}

#[cfg(not(target_os = "linux"))]
fn uninstall_systemd() -> Result<String> {
    bail!("the systemd backend is not compiled into this build")
}

/// Reads `StartInterval` back out of the installed plist with a tiny
/// hand-rolled scan (no plist-parsing dependency for one integer).
fn installed_interval(plist_text: &str) -> Option<u64> {
    let key_pos = plist_text.find("<key>StartInterval</key>")?;
    let rest = &plist_text[key_pos..];
    let start = rest.find("<integer>")? + "<integer>".len();
    let end = rest[start..].find("</integer>")? + start;
    rest[start..end].trim().parse().ok()
}

/// Reads `ProgramArguments` roots (every argument after `observe`) back
/// out of the installed plist.
fn installed_roots(plist_text: &str) -> Vec<String> {
    let Some(array_start) = plist_text.find("<key>ProgramArguments</key>") else {
        return Vec::new();
    };
    let rest = &plist_text[array_start..];
    let Some(open) = rest.find("<array>") else {
        return Vec::new();
    };
    let Some(close) = rest.find("</array>") else {
        return Vec::new();
    };
    let block = &rest[open + "<array>".len()..close];
    let mut strings: Vec<String> = Vec::new();
    for line in block.lines() {
        let line = line.trim();
        if let Some(v) = line
            .strip_prefix("<string>")
            .and_then(|s| s.strip_suffix("</string>"))
        {
            strings.push(
                v.replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">"),
            );
        }
    }
    // strings[0] = executable, strings[1] = "observe", the rest are roots.
    strings.into_iter().skip(2).collect()
}

/// One completed `observe` run's outcome, appended to the log and (for
/// the most recent run) persisted alongside the growth store so the
/// report header can read it without parsing the log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunOutcome {
    pub observed_at: u64,
    pub wall_ms: u64,
    pub walked_total: u64,
    pub projects: usize,
    pub mode: String,
    pub outcome: String,
}

impl RunOutcome {
    fn to_log_line(&self) -> String {
        format!(
            "observed_at={} wall_ms={} walked_total={} projects={} mode={} outcome={}",
            self.observed_at,
            self.wall_ms,
            self.walked_total,
            self.projects,
            self.mode,
            self.outcome
        )
    }

    fn from_log_line(line: &str) -> Option<RunOutcome> {
        let mut observed_at = None;
        let mut wall_ms = None;
        let mut walked_total = None;
        let mut projects = None;
        let mut mode = None;
        let mut outcome = None;
        for field in line.split_whitespace() {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "observed_at" => observed_at = value.parse().ok(),
                "wall_ms" => wall_ms = value.parse().ok(),
                "walked_total" => walked_total = value.parse().ok(),
                "projects" => projects = value.parse().ok(),
                "mode" => mode = Some(value.to_string()),
                "outcome" => outcome = Some(value.to_string()),
                _ => {}
            }
        }
        Some(RunOutcome {
            observed_at: observed_at?,
            wall_ms: wall_ms?,
            walked_total: walked_total?,
            projects: projects?,
            mode: mode?,
            outcome: outcome?,
        })
    }
}

/// Appends one line to the observation log.
pub fn append_log(path: &Path, outcome: &RunOutcome) -> Result<()> {
    store::append_line(store::LogFile::Observations(path), &outcome.to_log_line())
        .with_context(|| format!("open {}", path.display()))?;
    Ok(())
}

/// Reads the last well-formed line of the observation log.
pub fn last_log_outcome(path: &Path) -> Option<RunOutcome> {
    let text = read_owned_string(path).ok()?;
    text.lines().rev().find_map(RunOutcome::from_log_line)
}

/// `<store>/scheduled_runs.parquet` (R18b): the most recent scheduled
/// run's outcome, one row. Replaces `last_run.json`.
fn scheduled_runs_path(store_dir: &Path) -> PathBuf {
    store_dir.join("scheduled_runs.parquet")
}

/// Persists the most recent run's summary alongside the growth store, so
/// the report header can read it without parsing the log.
pub fn write_last_run(store_dir: &Path, outcome: &RunOutcome) -> Result<()> {
    store::StoreDir::at(store_dir)?.create()?;
    let path = scheduled_runs_path(store_dir);
    crate::growth::columns::write_scheduled_run_rows(
        &path,
        &[crate::growth::columns::StoredScheduledRunRow {
            observed_at: outcome.observed_at,
            wall_ms: outcome.wall_ms,
            walked_total: outcome.walked_total,
            projects: outcome.projects as u64,
            mode: outcome.mode.clone(),
            outcome: outcome.outcome.clone(),
        }],
    )
    .with_context(|| format!("write {}", path.display()))
}

pub fn read_last_run(store_dir: &Path) -> Option<RunOutcome> {
    crate::growth::columns::read_scheduled_run_rows(&scheduled_runs_path(store_dir))
        .ok()?
        .into_iter()
        .next()
        .map(|r| RunOutcome {
            observed_at: r.observed_at,
            wall_ms: r.wall_ms,
            walked_total: r.walked_total,
            projects: r.projects as usize,
            mode: r.mode,
            outcome: r.outcome,
        })
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

pub fn format_ago(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// The report header line describing scheduled-observation state, shown
/// right after `observed_at=...`. `suggested_root` is used only in the
/// "no schedule" suggestion text.
pub fn header_line(store_dir: &Path, suggested_root: &Path, now: u64) -> String {
    let installed = match scheduling() {
        // Never suggest `swamp schedule --every ...` on a platform where
        // that command refuses. A header that advertises a command the
        // tool will not run is worse than one that says nothing.
        crate::platform::Scheduling::Unavailable { .. } => {
            return "no schedule (not available on this platform; run `swamp observe` from your own timer)"
                .to_string();
        }
        crate::platform::Scheduling::LaunchdUserAgent => fs_gate::exists(plist_path()),
        crate::platform::Scheduling::SystemdUser => crate::systemd_user::timer_installed(),
    };
    if !installed {
        return format!(
            "no schedule (swamp schedule --every 30m {})",
            suggested_root.display()
        );
    }
    match read_last_run(store_dir) {
        Some(run) if run.outcome == "ok" => {
            format!(
                "last observation {} ({}, {:.1} s)",
                format_ago(now, run.observed_at),
                run.mode,
                run.wall_ms as f64 / 1000.0
            )
        }
        Some(run) => {
            format!(
                "last observation {} ({})",
                format_ago(now, run.observed_at),
                run.outcome
            )
        }
        None => "schedule installed; no run recorded yet".to_string(),
    }
}

/// `swamp schedule` with no arguments: report installed/loaded
/// state, interval, roots, last run, and the next expected run.
pub fn status(store_dir: &Path) -> Result<String> {
    match scheduling() {
        crate::platform::Scheduling::Unavailable { reason, planned } => {
            // On a platform that cannot install one, reporting "not
            // installed / enable it with ..." would advertise a command
            // that refuses.
            let _ = store_dir;
            return Ok(format!(
                "Scheduled observation: not available on this platform\n  Reason:  {reason}\n  Planned: {planned}\n  Until then: run `swamp observe` from your own timer.\n"
            ));
        }
        crate::platform::Scheduling::SystemdUser => return status_systemd(store_dir),
        crate::platform::Scheduling::LaunchdUserAgent => {}
    }
    let plist = plist_path();
    if !fs_gate::exists(&plist) {
        return Ok(
            "Scheduled observation: not installed\n  Enable it with: swamp schedule --every 30m <root>\n"
                .to_string(),
        );
    }
    let text = read_owned_string(&plist).with_context(|| format!("read {}", plist.display()))?;
    let seconds = installed_interval(&text);
    let roots = installed_roots(&text);
    let mut out = String::new();
    out.push_str("Scheduled observation: installed\n");
    out.push_str(&format!("  Label:    {LABEL}\n"));
    out.push_str(&format!("  Plist:    {}\n", plist.display()));
    match seconds {
        Some(s) => out.push_str(&format!("  Interval: {}\n", format_interval(s))),
        None => out.push_str("  Interval: unreadable (the plist was edited by hand)\n"),
    }
    if roots.is_empty() {
        // No frozen roots baked into the plist (#42/#50): each fire runs
        // `observe` with no positional roots, which re-resolves the
        // configured scope fresh every time -- so a config edit (a new
        // `include`, a new `exclude`, a detector toggle) takes effect on
        // the very next scheduled run, not only after `schedule --every`
        // is run again.
        out.push_str("  Roots:    (configured scope, resolved fresh on every run)\n");
    } else {
        out.push_str(&format!("  Roots:    {}\n", roots.join(" ")));
    }

    match read_last_run(store_dir).or_else(|| last_log_outcome(&log_file())) {
        Some(run) => {
            let now = crate::entities::now();
            out.push_str(&format!(
                "  Last run: {} ({}, {} projects, {:.1} s)\n",
                format_ago(now, run.observed_at),
                run.outcome,
                run.projects,
                run.wall_ms as f64 / 1000.0
            ));
            if let Some(s) = seconds {
                let next = run.observed_at + s;
                let now = crate::entities::now();
                if next > now {
                    out.push_str(&format!("  Next run: in {}\n", format_duration(next - now)));
                } else {
                    out.push_str("  Next run: due\n");
                }
            }
        }
        None => out.push_str("  Last run: none recorded yet\n"),
    }
    Ok(out)
}

#[cfg(target_os = "linux")]
fn status_systemd(store_dir: &Path) -> Result<String> {
    let mut out = crate::systemd_user::status(
        &mut crate::systemd_user::RealSystemctl,
        &crate::systemd_user::unit_dir()?,
    )?;
    match read_last_run(store_dir).or_else(|| last_log_outcome(&log_file())) {
        Some(run) => out.push_str(&format!(
            "  Last run: {} ({}, mode {}, {} projects, {:.1} s)\n",
            format_ago(crate::entities::now(), run.observed_at),
            run.outcome,
            run.mode,
            run.projects,
            run.wall_ms as f64 / 1000.0
        )),
        None => out.push_str("  Last run: none recorded yet\n"),
    }
    Ok(out)
}

#[cfg(not(target_os = "linux"))]
fn status_systemd(_store_dir: &Path) -> Result<String> {
    bail!("the systemd backend is not compiled into this build")
}

/// A single-flight lock so a scheduled run and a manual `observe` cannot
/// interleave. Backed by a plain lock file under the store directory
/// holding `pid<TAB>started_at`; not `flock` because the loser needs to
/// print a friendly message rather than block.
pub struct LockGuard {
    store: store::StoreDir,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = store::ObserveLock { store: &self.store }.remove();
    }
}

fn lock_path(store_dir: &Path) -> PathBuf {
    store_dir.join("observe.lock")
}

/// Whether the lock holder is still running: `kill(pid, 0)`, no
/// process started. It used to run the `kill` program, which a minimal
/// Linux image (the CI fleet's) does not have, so a live holder read as
/// dead and its lock was reclaimed under it.
fn pid_alive(pid: u32) -> bool {
    fs_gate::pid_alive(pid)
}

/// Result of trying to take the single-flight observation lock.
pub enum LockOutcome {
    Acquired(LockGuard),
    HeldBy { pid: u32, since: u64 },
}

/// Attempts to take the lock. A stale lock (owner pid no longer alive) is
/// reclaimed automatically.
pub fn acquire_lock(store_dir: &Path) -> Result<LockOutcome> {
    let store_dir_typed = store::StoreDir::at(store_dir)?;
    store_dir_typed.create()?;
    let path = lock_path(store_dir);

    loop {
        let pid = std::process::id();
        let since = crate::entities::now();
        match (store::ObserveLock {
            store: &store_dir_typed,
        })
        .create(&format!("{pid}\t{since}\n"))
        {
            Ok(()) => {
                return Ok(LockOutcome::Acquired(LockGuard {
                    store: store_dir_typed,
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let contents = read_owned_string(&path).unwrap_or_default();
                let mut parts = contents.trim().splitn(2, '\t');
                let pid: Option<u32> = parts.next().and_then(|p| p.parse().ok());
                let since: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                match pid {
                    Some(pid) if pid_alive(pid) => {
                        return Ok(LockOutcome::HeldBy { pid, since });
                    }
                    _ => {
                        // Stale lock: owner is gone or unparsable. Reclaim
                        // and retry once.
                        let _ = store::ObserveLock {
                            store: &store_dir_typed,
                        }
                        .remove(); // our own stale lock, owner pid confirmed dead
                        continue;
                    }
                }
            }
            Err(e) => return Err(e).context("create observe lock"),
        }
    }
}

/// Who holds the observation lock, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockHolder {
    pub pid: u32,
    pub since: u64,
}

/// Reads the lock's holder without taking, creating or removing the
/// lock. `None` when nobody live holds it (no file, unparsable, or the
/// owner pid is gone -- a stale file is left for `acquire_lock` to
/// reclaim). Spawns one `kill -0`: never call it from a UI event thread.
pub fn peek_lock(store_dir: &Path) -> Option<LockHolder> {
    let contents = read_owned_string(lock_path(store_dir)).ok()?;
    let mut parts = contents.trim().splitn(2, '\t');
    let pid: u32 = parts.next()?.parse().ok()?;
    let since: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    pid_alive(pid).then_some(LockHolder { pid, since })
}

/// `1m 12s` style duration for live status text.
pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_and_formats_intervals() {
        assert_eq!(parse_interval("30m").unwrap(), 1800);
        assert_eq!(parse_interval("1h").unwrap(), 3600);
        assert!(parse_interval("").is_err());
        assert!(parse_interval("bogus").is_err());
        assert!(parse_interval("0").is_err());
        assert_eq!(format_interval(1800), "30m");
        assert_eq!(format_interval(3600), "1h");
        assert_eq!(format_interval(86400), "1d");
        assert_eq!(format_interval(90), "90s");
    }

    #[test]
    fn plist_golden_content() {
        let exe = PathBuf::from("/usr/local/bin/swamp");
        let roots = vec![PathBuf::from("/Users/test/src")];
        let log = PathBuf::from("/Users/test/Library/Logs/swamp/observe.log");
        let body = render_plist(&exe, &roots, 1800, &log, Some("/tmp/store"));
        assert!(body.contains(&format!("<string>{LABEL}</string>")));
        assert!(body.contains("<string>/usr/local/bin/swamp</string>"));
        assert!(body.contains("<string>observe</string>"));
        assert!(body.contains("<string>/Users/test/src</string>"));
        assert!(body.contains("<integer>1800</integer>"));
        assert!(body.contains("<string>Background</string>"));
        assert!(body.contains("<key>LowPriorityIO</key>\n<true/>"));
        assert!(body.contains("<key>Nice</key>\n<integer>10</integer>"));
        assert!(body.contains("<key>RunAtLoad</key>\n<false/>"));
        assert!(body.contains("SWAMP_DIR"));
        assert!(body.contains("/tmp/store"));
        assert!(body.contains(&log.to_string_lossy().to_string()));

        let seconds = installed_interval(&body);
        assert_eq!(seconds, Some(1800));
        let roots_read = installed_roots(&body);
        assert_eq!(roots_read, vec!["/Users/test/src".to_string()]);
    }

    #[test]
    fn log_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("observe.log");
        let outcome = RunOutcome {
            observed_at: 1_000,
            wall_ms: 4200,
            walked_total: 12345,
            projects: 4,
            mode: "full".to_string(),
            outcome: "ok".to_string(),
        };
        append_log(&log, &outcome).unwrap();
        let read = last_log_outcome(&log).unwrap();
        assert_eq!(read.observed_at, 1_000);
        assert_eq!(read.wall_ms, 4200);
        assert_eq!(read.mode, "full");
        assert_eq!(read.outcome, "ok");
    }

    #[test]
    fn lock_prevents_second_observer() {
        let tmp = tempfile::tempdir().unwrap();
        let first = acquire_lock(tmp.path()).unwrap();
        assert!(matches!(first, LockOutcome::Acquired(_)));
        let second = acquire_lock(tmp.path()).unwrap();
        match second {
            LockOutcome::HeldBy { pid, .. } => assert_eq!(pid, std::process::id()),
            LockOutcome::Acquired(_) => panic!("second acquire should not succeed"),
        }
        drop(first);
        let third = acquire_lock(tmp.path()).unwrap();
        assert!(matches!(third, LockOutcome::Acquired(_)));
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = lock_path(tmp.path());
        fs::write(&path, "999999999\t1\n").unwrap();
        let outcome = acquire_lock(tmp.path()).unwrap();
        assert!(matches!(outcome, LockOutcome::Acquired(_)));
    }

    // Tests below mutate process-global env vars (SWAMP_* dirs and
    // test mode) to point launchd/plist/log paths at a temp dir instead
    // of the real machine. Serialized so parallel `cargo test` threads
    // never observe each other's env var.
    // The crate-wide lock, not a module-local one: `platform`'s tests
    // unset `HOME`, which `home()` reads (see `crate::TEST_ENV_LOCK`).
    use crate::TEST_ENV_LOCK as ENV_LOCK;

    #[cfg(target_os = "macos")]
    #[test]
    fn header_line_reports_no_schedule_when_plist_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SWAMP_LAUNCH_AGENTS_DIR points somewhere with no plist.
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", tmp.path());
        }
        let store = tempfile::tempdir().unwrap();
        let line = header_line(store.path(), Path::new("/Users/test/src"), 0);
        assert!(line.contains("no schedule"));
        assert!(line.contains("swamp schedule --every 30m"));
        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
        }
    }

    /// launchd-shaped: it asserts the contents of a plist and the
    /// `launchctl` calls around it. Gated rather than deleted -- the
    /// contract it pins is real on the platform that has it, and the
    /// platform that does not gets its own assertions below.
    #[cfg(target_os = "macos")]
    #[test]
    fn off_removes_plist_and_issues_bootout_in_test_mode() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
            std::env::set_var("SWAMP_TEST_MODE", "1");
        }
        let plist = plist_path();
        fs::write(&plist, "placeholder").unwrap();
        assert!(plist.exists());

        let message = uninstall().unwrap();
        assert!(!plist.exists(), "uninstall must remove the plist file");
        assert!(message.contains("Removed the scheduled observation"));

        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
            std::env::remove_var("SWAMP_TEST_MODE");
        }
    }

    /// launchd-shaped: it asserts the contents of a plist and the
    /// `launchctl` calls around it. Gated rather than deleted -- the
    /// contract it pins is real on the platform that has it, and the
    /// platform that does not gets its own assertions below.
    #[cfg(target_os = "macos")]
    #[test]
    fn off_with_no_plist_still_reports_no_schedule() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
            std::env::set_var("SWAMP_TEST_MODE", "1");
        }
        let message = uninstall().unwrap();
        assert!(message.contains("No scheduled observation is installed"));
        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
            std::env::remove_var("SWAMP_TEST_MODE");
        }
    }

    /// launchd-shaped: it asserts the contents of a plist and the
    /// `launchctl` calls around it. Gated rather than deleted -- the
    /// contract it pins is real on the platform that has it, and the
    /// platform that does not gets its own assertions below.
    #[cfg(target_os = "macos")]
    #[test]
    fn status_parses_a_fixture_log_and_installed_plist() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
        }
        let plist = plist_path();
        let exe = PathBuf::from("/usr/local/bin/swamp");
        let body = render_plist(
            &exe,
            &[PathBuf::from("/Users/test/src")],
            1800,
            &PathBuf::from("/tmp/log"),
            None,
        );
        fs::write(&plist, body).unwrap();

        let store = tempfile::tempdir().unwrap();
        let outcome = RunOutcome {
            observed_at: 1_000,
            wall_ms: 4200,
            walked_total: 999,
            projects: 3,
            mode: "full".to_string(),
            outcome: "ok".to_string(),
        };
        write_last_run(store.path(), &outcome).unwrap();

        let text = status(store.path()).unwrap();
        assert!(text.contains("installed"));
        assert!(text.contains("30m"));
        assert!(text.contains("/Users/test/src"));
        assert!(text.contains("3 projects"));
        assert!(text.contains("4.2 s"));

        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
        }
    }

    /// #42/#50: `install` with no explicit roots must not bail (it used
    /// to require at least one), must write a plist whose
    /// `ProgramArguments` names no root at all beyond `observe` itself,
    /// and `status` must say so plainly rather than printing a blank
    /// "Roots:" line -- the tempting shortcut this guards against is
    /// resolving the scope once at install time and freezing the result
    /// into the plist, which would silently stop tracking a later config
    /// edit until `schedule --every` was run again.
    /// launchd-shaped: it asserts the contents of a plist and the
    /// `launchctl` calls around it. Gated rather than deleted -- the
    /// contract it pins is real on the platform that has it, and the
    /// platform that does not gets its own assertions below.
    #[cfg(target_os = "macos")]
    #[test]
    fn install_with_no_roots_freezes_nothing_and_status_says_so() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
            std::env::set_var("SWAMP_LOG_DIR", logs.path());
            std::env::set_var("SWAMP_TEST_MODE", "1");
        }

        let message = install("30m", &[], false).unwrap();
        assert!(message.contains("resolved fresh on every run"), "{message}");

        let plist_text = fs::read_to_string(plist_path()).unwrap();
        assert!(
            installed_roots(&plist_text).is_empty(),
            "no root should be frozen into the plist's argv: {plist_text}"
        );
        // The launched command is still exactly `<exe> observe` -- no
        // trailing empty-string argument sneaking in from an empty loop.
        let array_block = plist_text
            .split("<key>ProgramArguments</key>")
            .nth(1)
            .and_then(|s| s.split("</array>").next())
            .unwrap();
        assert_eq!(
            array_block.matches("<string>").count(),
            2,
            "exactly exe + \"observe\", no root strings: {array_block}"
        );

        let store = tempfile::tempdir().unwrap();
        let status_text = status(store.path()).unwrap();
        assert!(
            status_text.contains("(configured scope, resolved fresh on every run)"),
            "{status_text}"
        );

        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
            std::env::remove_var("SWAMP_LOG_DIR");
            std::env::remove_var("SWAMP_TEST_MODE");
        }
    }

    /// An explicit root list must still be frozen into the plist exactly
    /// as before -- only the *no-roots* case changed behavior.
    /// launchd-shaped: it asserts the contents of a plist and the
    /// `launchctl` calls around it. Gated rather than deleted -- the
    /// contract it pins is real on the platform that has it, and the
    /// platform that does not gets its own assertions below.
    #[cfg(target_os = "macos")]
    #[test]
    fn install_with_explicit_roots_still_freezes_them() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
            std::env::set_var("SWAMP_LOG_DIR", logs.path());
            std::env::set_var("SWAMP_TEST_MODE", "1");
        }

        install("30m", &[PathBuf::from("/Users/test/src")], false).unwrap();
        let plist_text = fs::read_to_string(plist_path()).unwrap();
        assert_eq!(
            installed_roots(&plist_text),
            vec!["/Users/test/src".to_string()]
        );

        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
            std::env::remove_var("SWAMP_LOG_DIR");
            std::env::remove_var("SWAMP_TEST_MODE");
        }
    }

    // ---------------------------------------------------------------
    // The platform that has no scheduler
    // ---------------------------------------------------------------

    /// Linux with no reachable user manager (no `$XDG_RUNTIME_DIR`: a
    /// container, cron, a non-login shell): `install` refuses, says what
    /// to do instead, and leaves nothing behind -- no unit, no log
    /// directory, and never a LaunchAgent. The tempting shortcut is to
    /// write the units anyway and report success for a timer that no
    /// manager will ever run.
    #[cfg(target_os = "linux")]
    #[test]
    fn install_without_a_user_manager_refuses_and_writes_nothing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        let units = tempfile::tempdir().unwrap();
        let saved = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
            std::env::set_var("SWAMP_LOG_DIR", logs.path());
            std::env::set_var("SWAMP_SYSTEMD_UNIT_DIR", units.path().join("user"));
            std::env::remove_var("XDG_RUNTIME_DIR");
        }

        let err = install("30m", &[], false).expect_err("no user manager: install must refuse");
        let message = format!("{err}");
        assert!(message.contains("no systemd user manager"), "{message}");
        assert!(
            message.contains("cron"),
            "the refusal says what to do instead: {message}"
        );
        assert!(
            !units.path().join("user").exists(),
            "a refused install created the unit dir"
        );
        assert!(std::fs::read_dir(agents.path()).unwrap().next().is_none());
        assert!(std::fs::read_dir(logs.path()).unwrap().next().is_none());
        assert!(
            !plist_path().exists(),
            "a Linux build never writes a LaunchAgent"
        );

        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
            std::env::remove_var("SWAMP_LOG_DIR");
            std::env::remove_var("SWAMP_SYSTEMD_UNIT_DIR");
            if let Some(v) = saved {
                std::env::set_var("XDG_RUNTIME_DIR", v);
            }
        }
    }

    /// With a user manager (test mode answers for it), the real entry
    /// point writes swamp's units into the user unit directory and
    /// nothing into a LaunchAgents directory; `--off` removes them.
    #[cfg(target_os = "linux")]
    #[test]
    fn install_and_off_through_the_real_entry_points_use_systemd_units_only() {
        let _guard = ENV_LOCK.lock().unwrap();
        let agents = tempfile::tempdir().unwrap();
        let units = tempfile::tempdir().unwrap();
        let saved = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe {
            std::env::set_var("SWAMP_LAUNCH_AGENTS_DIR", agents.path());
            std::env::set_var("SWAMP_SYSTEMD_UNIT_DIR", units.path());
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/4242");
            std::env::set_var("SWAMP_TEST_MODE", "1");
        }
        let text = install("1h", &[PathBuf::from("/home/dev/src")], true).unwrap();
        assert!(text.contains("every 1h"), "{text}");
        for n in [
            crate::systemd_user::SERVICE,
            crate::systemd_user::TIMER,
            crate::systemd_user::COLLECTOR,
        ] {
            assert!(units.path().join(n).exists(), "{n} written");
        }
        assert!(std::fs::read_dir(agents.path()).unwrap().next().is_none());
        let off = uninstall().unwrap();
        assert!(off.contains("Removed"), "{off}");
        assert!(std::fs::read_dir(units.path()).unwrap().next().is_none());
        unsafe {
            std::env::remove_var("SWAMP_LAUNCH_AGENTS_DIR");
            std::env::remove_var("SWAMP_SYSTEMD_UNIT_DIR");
            std::env::remove_var("SWAMP_TEST_MODE");
            match saved {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    /// macOS refuses the Linux-only collector rather than installing a
    /// resident process FSEvents makes unnecessary.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_refuses_a_collector() {
        let _guard = ENV_LOCK.lock().unwrap();
        let err = install("1h", &[], true).expect_err("macOS needs no collector");
        assert!(format!("{err}").contains("FSEvents"), "{err}");
    }

    /// Portable across both: whatever the platform, the log directory is
    /// derived from the platform's own convention and `SWAMP_LOG_DIR`
    /// overrides it. Neither platform's convention may appear in the
    /// other's build.
    #[test]
    fn the_log_directory_follows_this_platforms_convention_and_the_override_wins() {
        let _guard = ENV_LOCK.lock().unwrap();
        // HOME points at a temp dir: the hermeticity guard refuses a test
        // that derives the real user's log directory.
        let fake_home = tempfile::tempdir().unwrap();
        let real_home = std::env::var_os("HOME");
        unsafe {
            std::env::remove_var("SWAMP_LOG_DIR");
            std::env::set_var("HOME", fake_home.path());
        }
        let derived = log_dir();
        unsafe {
            match &real_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
        let text = derived.display().to_string();
        match crate::platform::Os::current() {
            crate::platform::Os::MacOs => {
                assert!(text.contains("Library/Logs/swamp"), "{text}");
                assert!(
                    !text.contains(".local/state"),
                    "an XDG state path on macOS: {text}"
                );
            }
            crate::platform::Os::Linux => {
                assert!(text.ends_with("swamp"), "{text}");
                assert!(
                    !text.contains("Library/Logs"),
                    "a macOS log path on Linux: {text}"
                );
                assert!(
                    text.contains(".local/state") || std::env::var_os("XDG_STATE_HOME").is_some(),
                    "Linux logs belong under $XDG_STATE_HOME, not data or cache: {text}"
                );
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("SWAMP_LOG_DIR", tmp.path());
        }
        assert_eq!(log_dir(), tmp.path());
        unsafe {
            std::env::remove_var("SWAMP_LOG_DIR");
        }
    }

    /// A relative `$XDG_STATE_HOME` is invalid per the XDG base
    /// directory spec and must be ignored, not joined to whatever
    /// directory the process happens to be in.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_relative_xdg_state_home_is_ignored() {
        let _guard = ENV_LOCK.lock().unwrap();
        let fake_home = tempfile::tempdir().unwrap();
        let real_home = std::env::var_os("HOME");
        unsafe {
            std::env::remove_var("SWAMP_LOG_DIR");
            std::env::set_var("XDG_STATE_HOME", "relative/state");
            std::env::set_var("HOME", fake_home.path());
        }
        let derived = log_dir();
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
            match &real_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
        assert!(
            derived.is_absolute(),
            "a relative XDG_STATE_HOME produced a relative log directory: {}",
            derived.display()
        );
        assert!(
            derived.display().to_string().contains(".local/state/swamp"),
            "{}",
            derived.display()
        );
    }

    /// Tempting wrong patch: truncate the log at the cap (losing the
    /// recent history `schedule status` falls back to) or rotate into
    /// numbered files without a limit. The log rotates into exactly one
    /// `observe.log.1`, the newest line is always in `observe.log`, and
    /// the two together never exceed twice the cap.
    #[test]
    fn the_observe_log_rotates_into_one_file_and_stays_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("observe.log");
        // Long lines so three caps' worth of history takes a few hundred
        // appends (each one syncs).
        let filler = "x".repeat(8 * 1024);
        let cap = store::LOG_CAP_BYTES;
        let appends = 3 * cap / (8 * 1024) + 10;
        for i in 0..appends {
            let outcome = RunOutcome {
                observed_at: 1_000 + i,
                wall_ms: 1,
                walked_total: 1,
                projects: 1,
                mode: format!("full{filler}"),
                outcome: "ok".to_string(),
            };
            append_log(&log, &outcome).unwrap();
        }
        let size = |p: &std::path::Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let rotated = tmp.path().join("observe.log.1");
        assert!(rotated.exists(), "the log never rotated");
        assert!(size(&log) <= cap + 9 * 1024, "{}", size(&log));
        assert!(size(&rotated) <= cap + 9 * 1024, "{}", size(&rotated));
        let names: Vec<_> = fs::read_dir(tmp.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 2, "more than one rotated file");
        assert_eq!(
            last_log_outcome(&log).unwrap().observed_at,
            1_000 + appends - 1
        );
    }

    #[test]
    fn timeout_outcome_writes_a_log_line() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("observe.log");
        let outcome = RunOutcome {
            observed_at: 5_000,
            wall_ms: 1_800_000,
            walked_total: 0,
            projects: 0,
            mode: "full".to_string(),
            outcome: "timeout".to_string(),
        };
        append_log(&log, &outcome).unwrap();
        let read = last_log_outcome(&log).unwrap();
        assert_eq!(read.outcome, "timeout");
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains("outcome=timeout"));
    }

    #[test]
    fn peek_lock_reads_holder_without_taking_it() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(peek_lock(tmp.path()), None);
        let LockOutcome::Acquired(guard) = acquire_lock(tmp.path()).unwrap() else {
            panic!("fresh store must acquire");
        };
        let holder = peek_lock(tmp.path()).expect("held");
        assert_eq!(holder.pid, std::process::id());
        // Peeking did not disturb the lock.
        assert!(matches!(
            acquire_lock(tmp.path()).unwrap(),
            LockOutcome::HeldBy { .. }
        ));
        drop(guard);
        assert_eq!(peek_lock(tmp.path()), None);
    }

    #[test]
    fn elapsed_format() {
        assert_eq!(format_elapsed(72), "1m 12s");
        assert_eq!(format_elapsed(5), "5s");
    }
}

/// How long a path an `observe` pass was stopped on is skipped (#190).
pub const STALL_QUARANTINE_SECS: u64 = 24 * 3600;

fn stalled_file(store_dir: &Path) -> Result<PathBuf> {
    Ok(store::TextFile::Stalled {
        store: &store::StoreDir::at(store_dir)?,
    }
    .path()?)
}

fn read_stalled(store_dir: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(path) = stalled_file(store_dir) else {
        return Vec::new();
    };
    let Ok(text) = read_owned_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let (at, p) = l.split_once('\t')?;
            Some((at.parse().ok()?, PathBuf::from(p)))
        })
        .collect()
}

/// Paths a pass was stopped on within the last [`STALL_QUARANTINE_SECS`],
/// with when.
pub fn quarantined(store_dir: &Path, now: u64) -> Vec<(u64, PathBuf)> {
    read_stalled(store_dir)
        .into_iter()
        .filter(|(at, _)| now.saturating_sub(*at) < STALL_QUARANTINE_SECS)
        .collect()
}

/// Records that a pass was stopped on `path` at `now`, dropping entries
/// past the quarantine.
pub fn record_stalled(store_dir: &Path, path: &Path, now: u64) -> Result<()> {
    let mut rows: Vec<(u64, PathBuf)> = quarantined(store_dir, now)
        .into_iter()
        .filter(|(_, p)| p != path)
        .collect();
    rows.push((now, path.to_path_buf()));
    let text: String = rows
        .iter()
        .map(|(at, p)| format!("{at}\t{}\n", p.display()))
        .collect();
    let store = store::StoreDir::at(store_dir)?;
    store.create()?;
    store::write_text(store::TextFile::Stalled { store: &store }, &text)
        .context("write stalled-paths.tsv")
}

/// `YYYY-MM-DD` (UTC) for unix seconds.
pub fn utc_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod stall_quarantine_tests {
    use super::*;

    /// A stalled path is skipped for a day, then retried; the file keeps
    /// one row per path. Tempting wrong patch: appending forever, so an
    /// old stall is never retried.
    #[test]
    fn a_stalled_path_is_quarantined_for_a_day_then_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Path::new("/x/Library/Caches");
        record_stalled(tmp.path(), p, 1_000).unwrap();
        record_stalled(tmp.path(), p, 2_000).unwrap();
        assert_eq!(
            quarantined(tmp.path(), 3_000),
            vec![(2_000, p.to_path_buf())]
        );
        assert!(quarantined(tmp.path(), 2_000 + STALL_QUARANTINE_SECS).is_empty());
        assert_eq!(utc_date(1_790_773_352), "2026-09-30");
    }
}
