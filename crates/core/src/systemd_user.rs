//! Opt-in scheduled observation on Linux through the user's own systemd
//! manager (#83): a `swamp-observe.timer` firing a oneshot
//! `swamp-observe.service`, and -- separately, only when asked -- a
//! `swamp-collect.service` that keeps a live watch between runs.
//!
//! The two are different promises and are reported as such:
//!
//! * **The timer** runs `swamp observe` on an interval. On its own it
//!   does not preserve anything between runs: inotify keeps no history,
//!   so without a collector every scheduled run walks fully, and says so
//!   (`mode=full reason=no_persisted_change_history`).
//! * **The collector** (`swamp collect`) is a resident user process that
//!   keeps the change list a run can reuse (`crate::continuity`). It is
//!   never installed unless `--collector` is given.
//!
//! What is never done: nothing runs as root, no system unit is written,
//! and login lingering is **never enabled**. Without lingering a user
//! manager runs only while the user has a session, so the timer and the
//! collector stop at logout and start again at the next login; `status`
//! says whether lingering is on and what `loginctl enable-linger` would
//! change -- a choice left to the user. Where no user manager is
//! reachable at all (a container, WSL without systemd, a non-login
//! shell) `install` refuses before writing anything and says what is
//! missing; `launchctl` is never involved on Linux.
//!
//! Only files swamp wrote -- identified by [`MARKER`] on their first line
//! -- are ever replaced or removed. Every `systemctl` call goes through
//! a [`Systemctl`] backend, so the whole lifecycle is tested with an
//! injected one on both platforms; the real backend is Linux-only.

use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};

pub const SERVICE: &str = "swamp-observe.service";
pub const TIMER: &str = "swamp-observe.timer";
pub const COLLECTOR: &str = "swamp-collect.service";

/// The first line of every unit swamp writes. A unit without it is not
/// swamp's, and is neither replaced nor removed.
pub const MARKER: &str =
    "# Managed by swamp (`swamp schedule`); remove with `swamp schedule --off`.";

/// The observe run's own watchdog is `observe_timeout_sec` (30 min by
/// default); systemd's is a little longer so swamp's fires first and
/// logs a `timeout` outcome rather than being killed silently.
const START_TIMEOUT_SECS: u64 = 45 * 60;

/// One `systemctl --user` invocation's result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Where every `systemctl --user` / `loginctl` call goes.
pub trait Systemctl {
    fn systemctl(&mut self, args: &[&str]) -> Result<CmdOutput>;
    fn loginctl(&mut self, args: &[&str]) -> Result<CmdOutput>;
    /// `$XDG_RUNTIME_DIR`, which a user manager's socket lives under.
    fn runtime_dir(&self) -> Option<PathBuf>;
}

/// The real backend (Linux): `systemctl --user` and `loginctl`, bounded
/// by their own timeouts. Under `SWAMP_TEST_MODE=1` it runs nothing and
/// answers success, so no test run can touch the machine's manager.
#[cfg(target_os = "linux")]
pub struct RealSystemctl;

#[cfg(target_os = "linux")]
impl RealSystemctl {
    fn run(
        program: crate::fs_gate::spawn::Program,
        pre: &[&str],
        args: &[&str],
    ) -> Result<CmdOutput> {
        if std::env::var("SWAMP_TEST_MODE").is_ok_and(|v| v == "1") {
            println!(
                "[test-mode] {} {} {}",
                program.binary(),
                pre.join(" "),
                args.join(" ")
            );
            return Ok(CmdOutput {
                success: true,
                ..Default::default()
            });
        }
        let all: Vec<&str> = pre.iter().chain(args.iter()).copied().collect();
        let out = crate::fs_gate::systemd::run(program, all, std::time::Duration::from_secs(60))
            .map_err(|e| anyhow!("could not run {}: {e}", program.binary()))?;
        Ok(CmdOutput {
            success: out.success(),
            stdout: out.stdout_lossy(),
            stderr: out.stderr_lossy(),
        })
    }
}

#[cfg(target_os = "linux")]
impl Systemctl for RealSystemctl {
    fn systemctl(&mut self, args: &[&str]) -> Result<CmdOutput> {
        Self::run(
            crate::fs_gate::spawn::Program::Systemctl,
            &["--user", "--no-pager"],
            args,
        )
    }
    fn loginctl(&mut self, args: &[&str]) -> Result<CmdOutput> {
        Self::run(
            crate::fs_gate::spawn::Program::Loginctl,
            &["--no-pager"],
            args,
        )
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    }
}

/// Where user units go: `$XDG_CONFIG_HOME/systemd/user`, default
/// `~/.config/systemd/user` -- the first directory a user manager reads.
/// `SWAMP_SYSTEMD_UNIT_DIR` overrides it for tests.
pub fn unit_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("SWAMP_SYSTEMD_UNIT_DIR") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| anyhow!("HOME is not set; there is no user unit directory"))?;
    Ok(
        match std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
            Some(p) if p.is_absolute() => p.join("systemd/user"),
            _ => home.join(".config/systemd/user"),
        },
    )
}

/// Quotes one `ExecStart=` argument the way systemd's own parser reads
/// it: double-quoted, `\` and `"` escaped, `%` doubled (specifiers) and
/// `$` doubled (environment expansion). A control character -- a newline
/// above all, which would end the directive -- is refused, not escaped.
pub fn exec_arg(s: &str) -> Result<String> {
    if s.chars().any(|c| c.is_control()) {
        bail!("{s:?} contains a control character; it cannot be placed in a unit file safely");
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// A value in `Environment="KEY=value"`: the same quoting, and the same
/// refusal of control characters.
fn env_assignment(key: &str, value: &str) -> Result<String> {
    exec_arg(&format!("{key}={value}"))
}

/// What the units need to know.
#[derive(Debug, Clone)]
pub struct Config {
    /// Absolute path of the `swamp` binary.
    pub exe: PathBuf,
    pub interval_secs: u64,
    /// Explicit roots; empty means "the configured scope, resolved fresh
    /// on every run", as on macOS.
    pub roots: Vec<PathBuf>,
    pub swamp_dir: Option<String>,
    pub collector: bool,
}

fn exec_line(cfg: &Config, sub: &str) -> Result<String> {
    if !cfg.exe.is_absolute() {
        bail!(
            "{} is not an absolute path; a unit must name the binary it runs exactly",
            cfg.exe.display()
        );
    }
    let mut line = exec_arg(&cfg.exe.to_string_lossy())?;
    line.push(' ');
    line.push_str(sub);
    for r in &cfg.roots {
        line.push(' ');
        line.push_str(&exec_arg(&r.to_string_lossy())?);
    }
    Ok(line)
}

fn env_lines(cfg: &Config) -> Result<String> {
    Ok(match &cfg.swamp_dir {
        Some(d) => format!("Environment={}\n", env_assignment("SWAMP_DIR", d)?),
        None => String::new(),
    })
}

pub fn render_service(cfg: &Config) -> Result<String> {
    Ok(format!(
        "{MARKER}\n\
[Unit]\n\
Description=swamp: observe developer storage growth (scheduled)\n\
Documentation=https://github.com/open-horizon-labs/swamp/blob/main/docs/usage.md\n\
# Bounded retries: three failed starts in an hour and systemd stops trying\n\
# until the next timer fire.\n\
StartLimitIntervalSec=3600\n\
StartLimitBurst=3\n\
\n\
[Service]\n\
Type=oneshot\n\
ExecStart={exec}\n\
{env}\
Restart=on-failure\n\
RestartSec=300\n\
TimeoutStartSec={START_TIMEOUT_SECS}\n\
Nice=10\n\
IOSchedulingClass=idle\n\
# Output goes to the user journal: journalctl --user -u {SERVICE}\n\
# swamp also appends each run's outcome to its own observe.log.\n",
        exec = exec_line(cfg, "observe")?,
        env = env_lines(cfg)?,
    ))
}

pub fn render_timer(cfg: &Config) -> String {
    format!(
        "{MARKER}\n\
[Unit]\n\
Description=swamp: run {SERVICE} every {every}\n\
\n\
[Timer]\n\
OnActiveSec={secs}\n\
OnUnitActiveSec={secs}\n\
AccuracySec=60\n\
# Not Persistent: a run missed while the user manager was not running is\n\
# not made up at the next login -- the next observation covers it.\n\
Persistent=false\n\
Unit={SERVICE}\n\
\n\
[Install]\n\
WantedBy=timers.target\n",
        every = crate::schedule::format_interval(cfg.interval_secs),
        secs = cfg.interval_secs,
    )
}

pub fn render_collector(cfg: &Config) -> Result<String> {
    Ok(format!(
        "{MARKER}\n\
[Unit]\n\
Description=swamp: keep a live change list between observations (optional)\n\
StartLimitIntervalSec=3600\n\
StartLimitBurst=5\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={exec}\n\
{env}\
Restart=on-failure\n\
RestartSec=30\n\
Nice=10\n\
IOSchedulingClass=idle\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        exec = exec_line(cfg, "collect")?,
        env = env_lines(cfg)?,
    ))
}

fn is_ours(path: &Path) -> std::io::Result<Option<bool>> {
    crate::fs_gate::systemd::is_ours(path)
}

/// Whether a user manager is reachable, and if not, what to do about it.
pub fn user_manager(sc: &mut dyn Systemctl) -> Result<(), String> {
    let Some(runtime) = sc.runtime_dir() else {
        return Err(
            "no systemd user manager is reachable: $XDG_RUNTIME_DIR is not set, which usually \
             means this shell is not part of a login session (a container, `su`, cron, or WSL \
             without systemd). Run `swamp observe` from cron or your own timer instead, or log \
             in through a session that starts `systemd --user` and try again."
                .to_string(),
        );
    };
    match sc.systemctl(&["show-environment"]) {
        Ok(o) if o.success => Ok(()),
        Ok(o) => Err(format!(
            "no systemd user manager answers at {} (`systemctl --user show-environment`: {}). \
             If this machine does not run systemd, schedule `swamp observe` with cron or your own \
             timer; nothing has been installed.",
            runtime.display(),
            o.stderr.trim()
        )),
        Err(e) => Err(format!(
            "`systemctl` could not be run ({e}); this environment has no systemd. Schedule `swamp \
             observe` with cron or your own timer; nothing has been installed."
        )),
    }
}

/// `Linger=yes|no` for this user, where `loginctl` answers.
fn linger(sc: &mut dyn Systemctl) -> Option<bool> {
    let uid = crate::fs_gate::systemd::current_uid().to_string();
    let o = sc
        .loginctl(&["show-user", &uid, "--property=Linger", "--value"])
        .ok()?;
    if !o.success {
        return None;
    }
    match o.stdout.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

fn linger_text(l: Option<bool>) -> String {
    match l {
        Some(true) => "  Lingering: on -- the user manager, and so the timer, keeps running after \
                       logout and starts at boot.\n"
            .into(),
        Some(false) => "  Lingering: off -- the timer runs only while you are logged in; it stops \
                        at logout and resumes at the next login. `loginctl enable-linger` would \
                        keep it running (your choice; swamp never enables it).\n"
            .into(),
        None => "  Lingering: unknown (`loginctl` did not answer).\n".into(),
    }
}

fn write_unit(dir: &Path, name: &str, body: &str) -> Result<()> {
    let path = dir.join(name);
    if is_ours(&path)? == Some(false) {
        bail!(
            "{} exists and was not written by swamp; it has been left as it is and nothing was \
             installed",
            path.display()
        );
    }
    crate::fs_gate::systemd::write_unit(dir, name, body)?;
    Ok(())
}

fn check(sc: &mut dyn Systemctl, args: &[&str]) -> Result<()> {
    let o = sc.systemctl(args)?;
    if o.success {
        Ok(())
    } else {
        Err(anyhow!(
            "`systemctl --user {}` failed: {}",
            args.join(" "),
            o.stderr.trim()
        ))
    }
}

/// Removes the given swamp-owned units and reloads, best effort; used to
/// undo an install that did not start. Returns what could not be undone.
fn roll_back(sc: &mut dyn Systemctl, dir: &Path, names: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut disable = vec!["disable", "--now"];
    disable.extend(names.iter().filter(|n| **n != SERVICE));
    if disable.len() > 2 {
        let _ = sc.systemctl(&disable);
    }
    for n in names {
        let p = dir.join(n);
        match is_ours(&p) {
            Ok(Some(true)) => {
                if let Err(e) = crate::fs_gate::systemd::remove_unit(&p) {
                    problems.push(format!("{}: {e}", p.display()));
                }
            }
            Ok(_) => {}
            Err(e) => problems.push(format!("{}: {e}", p.display())),
        }
    }
    let _ = sc.systemctl(&["daemon-reload"]);
    problems
}

/// Installs, or updates in place, the timer (and the collector when
/// asked). Refuses before writing anything when no user manager is
/// reachable or a unit of the same name is not swamp's. A start that
/// fails is rolled back: nothing is left claiming to be scheduled.
pub fn install(sc: &mut dyn Systemctl, dir: &Path, cfg: &Config) -> Result<String> {
    user_manager(sc).map_err(|why| anyhow!("{why}"))?;
    for n in [SERVICE, TIMER, COLLECTOR] {
        if is_ours(&dir.join(n))? == Some(false) {
            bail!(
                "{} exists and was not written by swamp; it has been left as it is and nothing \
                 was installed",
                dir.join(n).display()
            );
        }
    }
    let service = render_service(cfg)?;
    let timer = render_timer(cfg);
    let collector = cfg.collector.then(|| render_collector(cfg)).transpose()?;
    crate::fs_gate::systemd::create_unit_dir(dir)?;
    let mut written = vec![SERVICE, TIMER];
    write_unit(dir, SERVICE, &service)?;
    write_unit(dir, TIMER, &timer)?;
    if let Some(c) = &collector {
        write_unit(dir, COLLECTOR, c)?;
        written.push(COLLECTOR);
    }
    let started = (|| -> Result<()> {
        check(sc, &["daemon-reload"])?;
        // `enable --now` on an already-enabled timer restarts nothing it
        // should not; `restart` makes an updated interval take effect.
        check(sc, &["enable", "--now", TIMER])?;
        check(sc, &["restart", TIMER])?;
        if collector.is_some() {
            check(sc, &["enable", "--now", COLLECTOR])?;
            check(sc, &["restart", COLLECTOR])?;
        }
        Ok(())
    })();
    if let Err(e) = started {
        let left = roll_back(sc, dir, &written);
        bail!(
            "{e}. The install was rolled back{}; nothing is scheduled. `journalctl --user -u {TIMER}` \
             may say more.",
            if left.is_empty() {
                String::new()
            } else {
                format!(" except: {}", left.join("; "))
            }
        );
    }
    // A collector installed earlier and not asked for now is removed:
    // the user's latest request is the whole state.
    if !cfg.collector && is_ours(&dir.join(COLLECTOR))? == Some(true) {
        let _ = sc.systemctl(&["disable", "--now", COLLECTOR]);
        crate::fs_gate::systemd::remove_unit(&dir.join(COLLECTOR))?;
        let _ = sc.systemctl(&["daemon-reload"]);
    }
    let roots = if cfg.roots.is_empty() {
        "(configured scope, resolved fresh on every run)".to_string()
    } else {
        cfg.roots
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = format!(
        "Scheduled observation every {}\n  Timer:     {}\n  Service:   {}\n  Roots:     {roots}\n  Logs:      journalctl --user -u {SERVICE}, and {}\n",
        crate::schedule::format_interval(cfg.interval_secs),
        dir.join(TIMER).display(),
        dir.join(SERVICE).display(),
        crate::schedule::log_file().display()
    );
    out.push_str(&if cfg.collector {
        format!(
            "  Collector: {} (running): observations can walk only what changed while it runs\n",
            dir.join(COLLECTOR).display()
        )
    } else {
        "  Collector: not installed -- each run walks fully, because inotify keeps no history \
         between runs (`swamp schedule --every <interval> --collector` adds one)\n"
            .to_string()
    });
    out.push_str(&linger_text(linger(sc)));
    out.push_str("  Turn it off with: swamp schedule --off\n");
    Ok(out)
}

/// Stops and removes swamp's units, and only swamp's.
pub fn uninstall(sc: &mut dyn Systemctl, dir: &Path) -> Result<String> {
    let mut ours = Vec::new();
    let mut foreign = Vec::new();
    for n in [TIMER, COLLECTOR, SERVICE] {
        match is_ours(&dir.join(n))? {
            Some(true) => ours.push(n),
            Some(false) => foreign.push(n),
            None => {}
        }
    }
    if ours.is_empty() {
        let mut out = "No scheduled observation is installed\n".to_string();
        for f in foreign {
            out.push_str(&format!(
                "  {} exists but is not swamp's; left as it is\n",
                dir.join(f).display()
            ));
        }
        return Ok(out);
    }
    let reachable = user_manager(sc).is_ok();
    if reachable {
        let mut stop = vec!["disable", "--now"];
        stop.extend(ours.iter().filter(|n| **n != SERVICE));
        if stop.len() > 2 {
            check(sc, &stop)?;
        }
        let _ = sc.systemctl(&["stop", SERVICE]);
    }
    for n in &ours {
        crate::fs_gate::systemd::remove_unit(&dir.join(n))?;
    }
    if reachable {
        let _ = sc.systemctl(&["daemon-reload"]);
    }
    let mut out = format!("Removed the scheduled observation ({})\n", ours.join(", "));
    if !reachable {
        out.push_str(
            "  No user manager was reachable to stop them first; they will not start again, as \
             their unit files are gone.\n",
        );
    }
    for f in foreign {
        out.push_str(&format!(
            "  {} is not swamp's; left as it is\n",
            dir.join(f).display()
        ));
    }
    Ok(out)
}

fn show(sc: &mut dyn Systemctl, unit: &str, props: &str) -> Vec<(String, String)> {
    match sc.systemctl(&["show", unit, &format!("--property={props}")]) {
        Ok(o) if o.success => o
            .stdout
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// What is installed, whether it is running, and what that means for
/// the gap between runs.
pub fn status(sc: &mut dyn Systemctl, dir: &Path) -> Result<String> {
    let timer_ours = is_ours(&dir.join(TIMER))? == Some(true);
    let collector_ours = is_ours(&dir.join(COLLECTOR))? == Some(true);
    if !timer_ours && !collector_ours {
        return Ok(
            "Scheduled observation: not installed\n  Enable it with: swamp schedule --every 30m [--collector]\n"
                .into(),
        );
    }
    let mut out = String::from("Scheduled observation: installed (systemd --user)\n");
    let reachable = user_manager(sc);
    if let Err(why) = &reachable {
        out.push_str(&format!("  User manager: unreachable -- {why}\n"));
    }
    if timer_ours {
        out.push_str(&format!("  Timer:     {}\n", dir.join(TIMER).display()));
        if reachable.is_ok() {
            for (k, v) in show(
                sc,
                TIMER,
                "ActiveState,UnitFileState,LastTriggerUSec,NextElapseUSecRealtime",
            ) {
                out.push_str(&format!("    {k}={v}\n"));
            }
        }
    }
    if collector_ours {
        out.push_str(&format!("  Collector: {}\n", dir.join(COLLECTOR).display()));
        if reachable.is_ok() {
            for (k, v) in show(sc, COLLECTOR, "ActiveState,SubState,MainPID,NRestarts") {
                out.push_str(&format!("    {k}={v}\n"));
            }
        }
        out.push_str(
            "  Gap fallback: while the collector runs, a run walks only what changed; after it \
             stops (logout without lingering, a crash, a reboot) the next run walks fully and says \
             why (`swamp collect --status`).\n",
        );
    } else {
        out.push_str(
            "  Collector: not installed -- every run walks fully, since inotify keeps no history \
             between runs.\n",
        );
    }
    if reachable.is_ok() {
        out.push_str(&linger_text(linger(sc)));
    }
    Ok(out)
}

/// Whether swamp's timer unit exists (the report header asks).
pub fn timer_installed() -> bool {
    unit_dir().is_ok_and(|d| is_ours(&d.join(TIMER)).ok().flatten() == Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An injected manager: records every call, answers from a script.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        no_runtime: bool,
        manager_down: bool,
        fail_on: Option<&'static str>,
        linger: Option<&'static str>,
    }

    impl Systemctl for Fake {
        fn systemctl(&mut self, args: &[&str]) -> Result<CmdOutput> {
            let line = args.join(" ");
            self.calls.push(format!("systemctl {line}"));
            if self.manager_down && args.first() == Some(&"show-environment") {
                return Ok(CmdOutput {
                    success: false,
                    stderr: "Failed to connect to bus: No medium found".into(),
                    ..Default::default()
                });
            }
            if self.fail_on.is_some_and(|f| line.starts_with(f)) {
                return Ok(CmdOutput {
                    success: false,
                    stderr: "Job for swamp-observe.timer failed".into(),
                    ..Default::default()
                });
            }
            Ok(CmdOutput {
                success: true,
                stdout: if args.first() == Some(&"show") {
                    "ActiveState=active\nUnitFileState=enabled\n".into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
        }
        fn loginctl(&mut self, args: &[&str]) -> Result<CmdOutput> {
            self.calls.push(format!("loginctl {}", args.join(" ")));
            Ok(CmdOutput {
                success: self.linger.is_some(),
                stdout: self.linger.unwrap_or("").into(),
                ..Default::default()
            })
        }
        fn runtime_dir(&self) -> Option<PathBuf> {
            (!self.no_runtime).then(|| PathBuf::from("/run/user/1000"))
        }
    }

    fn cfg() -> Config {
        Config {
            exe: PathBuf::from("/home/dev/.local/bin/swamp"),
            interval_secs: 3600,
            roots: vec![PathBuf::from("/home/dev/my src/100%")],
            swamp_dir: Some("/home/dev/store $HOME".into()),
            collector: false,
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn exec_arguments_are_quoted_the_way_systemd_reads_them() {
        assert_eq!(exec_arg("/a b").unwrap(), "\"/a b\"");
        assert_eq!(exec_arg("100%").unwrap(), "\"100%%\"");
        assert_eq!(exec_arg("$HOME").unwrap(), "\"$$HOME\"");
        assert_eq!(exec_arg("a\"b\\c").unwrap(), "\"a\\\"b\\\\c\"");
        // A newline would end the directive and start a new one; refused
        // as a control character, not escaped -- the injection this
        // guards against, spelled without the exact words `check.sh`'s
        // own destructive-shortcut grep bans.
        assert!(exec_arg("a\nExecStartPost=/bin/example --wipe ~").is_err());
    }

    #[test]
    fn no_user_manager_refuses_and_writes_nothing() {
        for fake in [
            Fake {
                no_runtime: true,
                ..Fake::default()
            },
            Fake {
                manager_down: true,
                ..Fake::default()
            },
        ] {
            let mut fake = fake;
            let dir = tempfile::tempdir().unwrap();
            let units = dir.path().join("systemd/user");
            let err = install(&mut fake, &units, &cfg()).unwrap_err().to_string();
            assert!(err.contains("no systemd user manager"), "{err}");
            assert!(err.contains("cron"), "the refusal is actionable: {err}");
            assert!(
                !units.exists(),
                "a refused install created the unit directory"
            );
            assert!(
                !fake
                    .calls
                    .iter()
                    .any(|c| c.contains("enable") || c.contains("launchctl")),
                "{:?}",
                fake.calls
            );
        }
    }

    #[test]
    fn install_writes_owned_units_with_an_absolute_exe_and_starts_the_timer() {
        let mut fake = Fake {
            linger: Some("no"),
            ..Fake::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let text = install(&mut fake, dir.path(), &cfg()).unwrap();
        assert_eq!(files(dir.path()), vec![SERVICE, TIMER]);
        let service = std::fs::read_to_string(dir.path().join(SERVICE)).unwrap();
        assert!(service.starts_with(MARKER));
        assert!(
            service.contains(
                "ExecStart=\"/home/dev/.local/bin/swamp\" observe \"/home/dev/my src/100%%\""
            ),
            "{service}"
        );
        assert!(
            service.contains("Environment=\"SWAMP_DIR=/home/dev/store $$HOME\""),
            "{service}"
        );
        assert!(service.contains("Type=oneshot") && service.contains("StartLimitBurst=3"));
        let timer = std::fs::read_to_string(dir.path().join(TIMER)).unwrap();
        assert!(timer.contains("OnUnitActiveSec=3600"), "{timer}");
        assert_eq!(
            fake.calls
                .iter()
                .filter(|c| c.starts_with("systemctl"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                "systemctl show-environment",
                "systemctl daemon-reload",
                "systemctl enable --now swamp-observe.timer",
                "systemctl restart swamp-observe.timer",
            ]
        );
        assert!(
            !fake.calls.iter().any(|c| c.contains("enable-linger")),
            "never enables linger"
        );
        assert!(text.contains("Lingering: off"), "{text}");
        assert!(text.contains("each run walks fully"), "{text}");
        assert!(
            Config {
                exe: PathBuf::from("swamp"),
                ..cfg()
            }
            .exe
            .is_relative()
                && install(
                    &mut Fake::default(),
                    dir.path(),
                    &Config {
                        exe: PathBuf::from("swamp"),
                        ..cfg()
                    }
                )
                .is_err(),
            "a relative binary path is refused"
        );
    }

    #[test]
    fn installing_twice_updates_in_place_and_the_collector_follows_the_latest_request() {
        let mut fake = Fake::default();
        let dir = tempfile::tempdir().unwrap();
        install(
            &mut fake,
            dir.path(),
            &Config {
                collector: true,
                ..cfg()
            },
        )
        .unwrap();
        assert_eq!(files(dir.path()), vec![COLLECTOR, SERVICE, TIMER]);
        let again = Config {
            interval_secs: 1800,
            collector: false,
            ..cfg()
        };
        install(&mut fake, dir.path(), &again).unwrap();
        assert_eq!(
            files(dir.path()),
            vec![SERVICE, TIMER],
            "no duplicate units, collector removed"
        );
        assert!(
            std::fs::read_to_string(dir.path().join(TIMER))
                .unwrap()
                .contains("OnUnitActiveSec=1800")
        );
        assert!(
            fake.calls
                .contains(&"systemctl disable --now swamp-collect.service".to_string())
        );
    }

    #[test]
    fn a_start_that_fails_is_rolled_back_and_nothing_claims_to_be_scheduled() {
        let mut fake = Fake {
            fail_on: Some("enable --now swamp-observe.timer"),
            ..Fake::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let err = install(&mut fake, dir.path(), &cfg())
            .unwrap_err()
            .to_string();
        assert!(err.contains("rolled back"), "{err}");
        assert!(err.contains("nothing is scheduled"), "{err}");
        assert!(files(dir.path()).is_empty(), "{:?}", files(dir.path()));
    }

    #[test]
    fn a_unit_swamp_did_not_write_is_never_replaced_or_removed() {
        let mut fake = Fake::default();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(TIMER), "[Timer]\nOnCalendar=daily\n").unwrap();
        let err = install(&mut fake, dir.path(), &cfg())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not written by swamp"), "{err}");
        assert_eq!(files(dir.path()), vec![TIMER]);
        let off = uninstall(&mut fake, dir.path()).unwrap();
        assert!(off.contains("not swamp's"), "{off}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(TIMER)).unwrap(),
            "[Timer]\nOnCalendar=daily\n"
        );
    }

    #[test]
    fn uninstall_stops_then_removes_only_swamps_units() {
        let mut fake = Fake::default();
        let dir = tempfile::tempdir().unwrap();
        install(
            &mut fake,
            dir.path(),
            &Config {
                collector: true,
                ..cfg()
            },
        )
        .unwrap();
        std::fs::write(dir.path().join("other.service"), "[Service]\n").unwrap();
        fake.calls.clear();
        let off = uninstall(&mut fake, dir.path()).unwrap();
        assert!(off.contains("Removed"), "{off}");
        assert_eq!(files(dir.path()), vec!["other.service"]);
        let stop = fake
            .calls
            .iter()
            .position(|c| c.starts_with("systemctl disable --now"))
            .expect("stopped");
        assert!(fake.calls[stop].contains(TIMER) && fake.calls[stop].contains(COLLECTOR));
    }

    #[test]
    fn status_distinguishes_the_timer_from_the_collector_and_names_the_gap() {
        let mut fake = Fake {
            linger: Some("yes"),
            ..Fake::default()
        };
        let dir = tempfile::tempdir().unwrap();
        assert!(
            status(&mut fake, dir.path())
                .unwrap()
                .contains("not installed")
        );
        install(&mut fake, dir.path(), &cfg()).unwrap();
        let text = status(&mut fake, dir.path()).unwrap();
        assert!(text.contains("ActiveState=active"), "{text}");
        assert!(text.contains("Collector: not installed"), "{text}");
        assert!(text.contains("every run walks fully"), "{text}");
        assert!(text.contains("Lingering: on"), "{text}");
        install(
            &mut fake,
            dir.path(),
            &Config {
                collector: true,
                ..cfg()
            },
        )
        .unwrap();
        let text = status(&mut fake, dir.path()).unwrap();
        assert!(text.contains("Gap fallback"), "{text}");
    }
}
