//! Simulator runtimes: `xcrun simctl runtime delete <UUID>`, previewed by
//! the same argv plus `--dry-run`.
//!
//! What simctl itself does not refuse, and swamp does (verified
//! 2026-09-30, xcrun 72, Xcode 27.0): it shuts booted simulators down and
//! deletes anyway, and it takes the alias `all` and set flags
//! (`--notUsedSinceDays`, `--outdated`) in the operand slot. So a booted
//! device on the runtime refuses, and the operand is only ever a UUID
//! `simctl runtime list -j` returned (`spawn::is_sim_runtime_uuid`).

use super::{
    Candidate, Host, Manager, Preview, Refusal, Size, Target, clean_block, digest, read_ok,
};
use crate::fs_gate::spawn::{ToolBin, is_sim_runtime_uuid};
use std::ffi::OsString;
use std::path::PathBuf;

/// The `xcrun --version` the dry-run format below was read from.
pub(super) const VERIFIED_VERSION: &str = "72";

/// CoreSimulator's own host process keeps a library open in every
/// mounted runtime; simctl stops it and unmounts before it deletes, so
/// it is not a holder that refuses.
const MANAGER_OWN: &[&str] = &["SimLaunchHost.arm64", "SimLaunchHost.x86_64", "SimLaunchHost"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Runtime {
    uuid: String,
    runtime_id: String,
    version: String,
    build: String,
    state: String,
    deletable: bool,
    size: Option<u64>,
    last_used: Option<String>,
    mount_path: Option<PathBuf>,
}

impl Runtime {
    fn label(&self) -> String {
        let platform = self
            .runtime_id
            .rsplit('.')
            .next()
            .and_then(|s| s.split('-').next())
            .unwrap_or("runtime");
        format!("{platform} {} ({})", self.version, self.build)
    }

    fn fact(&self) -> String {
        format!(
            "state {}, {}, sizeBytes {}, runtime {}",
            self.state,
            if self.deletable {
                "deletable"
            } else {
                "not deletable"
            },
            self.size
                .map_or_else(|| "absent".to_string(), |b| b.to_string()),
            self.runtime_id
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Device {
    name: String,
    state: String,
}

pub(super) fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|t| t.trim_end_matches('.').to_string())
}

fn parse_runtimes(text: &str) -> Result<Vec<Runtime>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("its JSON did not parse ({e})"))?;
    let obj = v.as_object().ok_or("its JSON is not a map of runtimes")?;
    let mut out = Vec::new();
    for (uuid, r) in obj {
        let s = |k: &str| r.get(k).and_then(|x| x.as_str()).map(str::to_string);
        out.push(Runtime {
            uuid: uuid.clone(),
            runtime_id: s("runtimeIdentifier").unwrap_or_default(),
            version: s("version").unwrap_or_default(),
            build: s("build").unwrap_or_default(),
            state: s("state").unwrap_or_default(),
            deletable: r.get("deletable").and_then(|d| d.as_bool()) == Some(true),
            size: r.get("sizeBytes").and_then(|b| b.as_u64()),
            last_used: s("lastUsedAt"),
            mount_path: s("mountPath").map(PathBuf::from),
        });
    }
    out.sort_by_key(|r| r.label());
    Ok(out)
}

fn parse_devices(text: &str, runtime_id: &str) -> Result<Vec<Device>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("its JSON did not parse ({e})"))?;
    let devices = v
        .get("devices")
        .and_then(|d| d.as_object())
        .ok_or("its JSON has no devices map")?;
    let mut out: Vec<Device> = devices
        .get(runtime_id)
        .and_then(|d| d.as_array())
        .map(|list| {
            list.iter()
                .map(|d| Device {
                    name: d
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unnamed")
                        .to_string(),
                    state: d
                        .get("state")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn list_runtimes(bin: &ToolBin) -> Result<Vec<Runtime>, Refusal> {
    let out = read_ok(
        bin,
        &["simctl", "runtime", "list", "-j"],
        Manager::Simulator,
        "runtime list",
    )?;
    if !out.success() {
        return Err(Refusal::new(
            "simctl runtime list did not succeed.",
            "Run `xcrun simctl runtime list` yourself to see why.",
        )
        .with_output(clean_block(&out.stderr)));
    }
    parse_runtimes(&out.stdout_lossy()).map_err(|why| {
        Refusal::new(
            format!("simctl's runtime list could not be read: {why}."),
            "Check the Xcode version; swamp runs nothing it cannot read.",
        )
    })
}

fn list_devices(bin: &ToolBin, runtime_id: &str) -> Result<Vec<Device>, Refusal> {
    let out = read_ok(
        bin,
        &["simctl", "list", "devices", "-j"],
        Manager::Simulator,
        "list devices",
    )?;
    if !out.success() {
        return Err(Refusal::new(
            "simctl list devices did not succeed, so swamp cannot tell whether a simulator is \
             booted on this runtime.",
            "Review again; swamp does not remove without that check.",
        ));
    }
    parse_devices(&out.stdout_lossy(), runtime_id).map_err(|why| {
        Refusal::new(
            format!("simctl's device list could not be read: {why}."),
            "Review again; swamp does not remove without that check.",
        )
    })
}

fn size_text(r: &Runtime) -> String {
    r.size.map_or_else(
        || "size not reported".to_string(),
        crate::render::human_bytes_pub,
    )
}

pub(super) fn candidates(bin: &ToolBin) -> Result<Vec<Candidate>, Refusal> {
    let runtimes = list_runtimes(bin)?;
    Ok(runtimes
        .into_iter()
        .filter(|r| is_sim_runtime_uuid(&r.uuid))
        .map(|r| Candidate {
            label: r.label(),
            facts: format!(
                "{} (simctl's sizeBytes), last used {}, {}",
                size_text(&r),
                r.last_used
                    .as_deref()
                    .map(|d| format!("{} (simctl's record)", d.get(..10).unwrap_or(d)))
                    .unwrap_or_else(|| "no record (simctl)".to_string()),
                r.state
            ),
            path: r.mount_path.clone(),
            target: Target::SimRuntime { uuid: r.uuid },
        })
        .collect())
}

pub(super) fn review(host: &Host, bin: &ToolBin, target: &Target) -> Result<Preview, Refusal> {
    let Target::SimRuntime { uuid } = target else {
        return Err(Refusal::new(
            "not a simulator runtime target",
            "Review it from the mise row.",
        ));
    };
    if !is_sim_runtime_uuid(uuid) {
        return Err(Refusal::new(
            format!("`{uuid}` is not a runtime UUID; swamp passes simctl nothing else."),
            "Review again from the list.",
        ));
    }
    let runtimes = list_runtimes(bin)?;
    let Some(rt) = runtimes.iter().find(|r| &r.uuid == uuid) else {
        return Err(Refusal::new(
            format!("simctl no longer lists runtime {uuid}."),
            "Press R to refresh, then review again.",
        ));
    };
    let label = rt.label();
    if !rt.deletable {
        return Err(Refusal::new(
            format!("simctl reports {label} is not deletable."),
            "Manage it in Xcode > Settings > Components.",
        ));
    }
    if !matches!(rt.state.as_str(), "Ready" | "Unusable") {
        return Err(Refusal::new(
            format!(
                "simctl reports {label} as {}; swamp removes only a Ready or Unusable runtime.",
                rt.state
            ),
            "Wait for simctl to finish with it, then review again.",
        ));
    }
    let devices = list_devices(bin, &rt.runtime_id)?;
    let booted: Vec<&str> = devices
        .iter()
        .filter(|d| d.state == "Booted")
        .map(|d| d.name.as_str())
        .collect();
    if !booted.is_empty() {
        return Err(Refusal::new(
            format!(
                "Simulator {} is booted on this runtime; simctl would shut it down and delete \
                 anyway.",
                booted.join(", ")
            ),
            "Shut it down in Simulator, then review again.",
        ));
    }
    let exec: Vec<OsString> = ["simctl", "runtime", "delete", uuid.as_str()]
        .iter()
        .map(OsString::from)
        .collect();
    let mut dry = exec.clone();
    dry.push(OsString::from("--dry-run"));
    let dry_args: Vec<&str> = dry.iter().filter_map(|a| a.to_str()).collect();
    let out = read_ok(
        bin,
        &dry_args,
        Manager::Simulator,
        "runtime delete --dry-run",
    )?;
    let mut shown = clean_block(&out.stdout);
    shown.extend(clean_block(&out.stderr));
    let shown = super::bound_lines(shown);
    if !out.success() {
        return Err(Refusal::new(
            format!(
                "simctl's dry run exited {}.",
                out.code
                    .map_or_else(|| "on a signal".to_string(), |c| c.to_string())
            ),
            "Read its output below; swamp runs nothing.",
        )
        .with_output(shown));
    }
    let lines: Vec<String> = clean_block(&out.stdout)
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let names_this = lines.len() == 1
        && lines[0].starts_with("Would delete ")
        && lines[0].split_whitespace().any(|w| w == uuid);
    if !names_this {
        return Err(Refusal::new(
            format!(
                "simctl's dry run did not name exactly this runtime (it printed {} line(s)).",
                lines.len()
            ),
            "Read its output below; swamp runs nothing.",
        )
        .with_output(shown));
    }
    let paths: Vec<PathBuf> = rt.mount_path.iter().cloned().collect();
    let open_files = super::open_files_fact(host, &paths, MANAGER_OWN, Manager::Simulator)?;
    let mut warnings = Vec::new();
    if !devices.is_empty() {
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        let shown_names = if names.len() > 6 {
            format!("{} and {} more", names[..6].join(", "), names.len() - 6)
        } else {
            names.join(", ")
        };
        warnings.push(format!(
            "{} simulator device{} on this runtime stop working without it (you can create \
             them again): {shown_names}.",
            devices.len(),
            if devices.len() == 1 { "" } else { "s" }
        ));
    }
    let devices_fact = if devices.is_empty() {
        "none".to_string()
    } else {
        devices
            .iter()
            .map(|d| format!("{} ({})", d.name, d.state))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let state = vec![
        (format!("simctl's entry for {uuid}"), rt.fact()),
        (format!("devices on {}", rt.runtime_id), devices_fact),
    ];
    let platform = label.split(' ').next().unwrap_or("iOS").to_string();
    Ok(Preview {
        manager: Manager::Simulator,
        target: target.clone(),
        bin: bin.clone(),
        dry_output_digest: digest(&shown),
        dry_output: shown,
        exec_argv: exec,
        dry_argv: dry,
        removes: vec![uuid.clone()],
        listed_as: vec![uuid.clone()],
        state,
        size: rt.size.map(|bytes| Size {
            bytes,
            source: "simctl's sizeBytes",
        }),
        regen: format!(
            "Reinstall is a download of {} (Xcode > Settings > Components, or `xcodebuild \
             -downloadPlatform {platform}`); whether Apple still offers this version is not \
             checked.",
            size_text(rt)
        ),
        evidence: vec![format!(
            "simctl's last use record: {}.",
            rt.last_used.as_deref().unwrap_or("no record")
        )],
        warnings,
        open_files,
        manager_version: String::new(),
        title: label,
    })
}

/// After a removal: whether simctl still lists the runtime.
pub(super) fn still_listed(bin: &ToolBin, preview: &Preview) -> Result<Vec<String>, String> {
    let runtimes = list_runtimes(bin).map_err(|r| r.reason)?;
    Ok(preview
        .listed_as
        .iter()
        .filter(|u| runtimes.iter().any(|r| &r.uuid == *u))
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_list_and_devices_parse() {
        let json = r#"{"5FF350CD-0800-4015-B796-BE66B16D154E":{"build":"23C54","deletable":true,"identifier":"5FF350CD-0800-4015-B796-BE66B16D154E","mountPath":"/Library/Developer/CoreSimulator/Volumes/iOS_23C54","runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-2","sizeBytes":8381044573,"state":"Ready","version":"26.2"}}"#;
        let r = parse_runtimes(json).unwrap();
        assert_eq!(r[0].label(), "iOS 26.2 (23C54)");
        assert_eq!(r[0].size, Some(8_381_044_573));
        assert!(r[0].last_used.is_none());
        let devices = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-2":[{"name":"iPhone 17 Pro","state":"Booted"},{"name":"iPhone Air","state":"Shutdown"}]}}"#;
        let d = parse_devices(devices, "com.apple.CoreSimulator.SimRuntime.iOS-26-2").unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(parse_version("xcrun version 72.\n").as_deref(), Some("72"));
    }
}
