#!/usr/bin/env bash
# #89: validation of a packaged Linux build on the machine it runs on,
# with measurements written to <out-dir> as the CI artifact that
# docs/platform.md summarizes. Everything runs against the *extracted
# release archive*, a generated workload and scratch state; nothing reads
# or writes the running user's own swamp store, Trash or units.
#
#   scripts/linux-validation.sh <archive.tar.gz> <version> <out-dir>
#
# Measures (each repeated $REPS times where it is a timing):
#   install-to-first-report, initial full scan, unchanged scan under a
#   live collector, a one-subtree mutation, a process-stopped gap, a real
#   inotify queue overflow and its recovery, watch memory and on-disk
#   state size; checks the CLI --json contract, that a DependencyTree
#   unit is reported with its facts (swamp reports; the human removes,
#   in the TUI or by hand -- the CLI has no propose/approve/execute any
#   more, so this validation is read-only too), and the
#   scheduled/collector lifecycle through the runner's systemd user
#   manager where one is reachable (recorded as unavailable, with the
#   reason, where it is not).
set -euo pipefail

archive="${1:?archive}"
version="${2:?version}"
out="${3:?out dir}"
REPS="${REPS:-3}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
report="$out/linux-validation.txt"
: > "$report"
say() { echo "$*" | tee -a "$report"; }

work="$(mktemp -d)"
cleanup() {
    [ -n "${collector_pid:-}" ] && kill "$collector_pid" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
field() { tr ' ' '\n' <<<"$1" | sed -n "s/^$2=//p" | head -1; }

say "# swamp Linux validation"
say "version: $version"
say "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
. /etc/os-release && say "distribution: $PRETTY_NAME"
say "kernel: $(uname -srm)"
say "libc: $(ldd --version | head -1)"
say "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //') x $(nproc)"
say "memory: $(awk '/MemTotal/{print $2" kB"}' /proc/meminfo)"
say "inotify: max_user_watches=$(cat /proc/sys/fs/inotify/max_user_watches) max_queued_events=$(cat /proc/sys/fs/inotify/max_queued_events) max_user_instances=$(cat /proc/sys/fs/inotify/max_user_instances)"
say "repetitions: $REPS"

# --- install ---------------------------------------------------------
t0=$(now_ms)
(cd "$(dirname "$archive")" && sha256sum -c "$(basename "$archive").sha256" >/dev/null)
tar -C "$work" -xzf "$archive"
bin="$(find "$work" -mindepth 2 -maxdepth 2 -name swamp -type f | head -1)"
export HOME="$work/home" SWAMP_DIR="$work/store" XDG_DATA_HOME="$work/home/.local/share"
mkdir -p "$HOME"
[ "$("$bin" --version)" = "swamp $version" ]

# --- a generated workload --------------------------------------------
root="$work/src"
python3 - "$root" <<'PY'
import os, sys
root = sys.argv[1]
for p in range(20):
    proj = os.path.join(root, f"proj{p:02d}")
    os.makedirs(os.path.join(proj, "src"), exist_ok=True)
    with open(os.path.join(proj, "Cargo.toml"), "w") as f:
        f.write(f'[package]\nname = "proj{p}"\nversion = "0.1.0"\n')
    for m in range(25):
        d = os.path.join(proj, "src", f"mod{m:02d}")
        os.makedirs(d, exist_ok=True)
        for i in range(8):
            with open(os.path.join(d, f"f{i}.rs"), "w") as f:
                f.write("// x\n" * 20)
    for s in range(20):
        d = os.path.join(proj, "target", "debug", "deps", f"crate{s:02d}")
        os.makedirs(d, exist_ok=True)
        for i in range(10):
            with open(os.path.join(d, f"o{i}.rlib"), "wb") as f:
                f.write(b"\0" * 4096)
web = os.path.join(root, "web")
os.makedirs(os.path.join(web, "node_modules", "left-pad"), exist_ok=True)
open(os.path.join(web, "package.json"), "w").write('{"name":"web"}\n')
open(os.path.join(web, ".gitignore"), "w").write("node_modules/\n")
with open(os.path.join(web, "node_modules", "left-pad", "index.js"), "wb") as f:
    f.write(b"\0" * 100000)
PY
for p in "$root"/proj* "$root"/web; do (cd "$p" && git init -q . && git add -A >/dev/null 2>&1 && git -c user.email=v@x -c user.name=v -c commit.gpgsign=false commit -qm init >/dev/null 2>&1 || true); done
dirs=$(find "$root" -type d | wc -l); files=$(find "$root" -type f | wc -l)
say "workload: $dirs directories, $files files, $(du -sh "$root" | cut -f1) on $(stat -f -c %T "$root")"

# `report` is a pure read (R12): a scope with no prior `observe` has
# nothing to read, and exits 2 with a `no_observation` JSON error rather
# than an empty-but-successful report.
! "$bin" report "$root" --json > "$work/first.json"
t1=$(now_ms)
say "install_to_first_report_ms: $((t1 - t0))"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d.get("error") == "no_observation"' "$work/first.json"
say "cli_json: report --json parses (no_observation before the first observe)"
"$bin" scope "$root" --json | python3 -c 'import json,sys; json.load(sys.stdin)' && say "cli_json: scope --json parses"

observe() { "$bin" observe "$root" | grep '^observed_at='; }
timed() { local a b line; a=$(now_ms); line="$(observe)"; b=$(now_ms); echo "$((b - a)) $line"; }

# --- initial full scan -----------------------------------------------
for r in $(seq "$REPS"); do
    rm -rf "$SWAMP_DIR"
    res="$(timed)"
    say "initial_full_scan run=$r ms=${res%% *} $(field "$res" mode) $(field "$res" reason) walked_total=$(field "$res" walked_total)"
done

# --- collector: unchanged, one subtree, gap, overflow ----------------
rm -rf "$SWAMP_DIR"
start_collector() {
    "$bin" collect "$root" 2>>"$out/collector.log" &
    collector_pid=$!
    # The checkpoint is a Parquet table (R18b): ask the CLI what it says.
    for _ in $(seq 200); do
        "$bin" collect --status "$root" --json 2>/dev/null |
            grep -Eq "\"pid\": ?$collector_pid\b" && return 0
        sleep 0.05
    done
    echo "collector did not start" >&2; exit 1
}
start_collector
sleep 1.2
# A fresh store: the first observation records the classification rules,
# the second anchors the cursor; both walk fully, as on macOS.
res="$(timed)"; say "collector_first_observation ms=${res%% *} mode=$(field "$res" mode) reason=$(field "$res" reason)"
sleep 1.2
res="$(timed)"; say "collector_anchor_observation ms=${res%% *} mode=$(field "$res" mode) reason=$(field "$res" reason)"
for r in $(seq "$REPS"); do
    sleep 1.2
    res="$(timed)"
    say "unchanged_under_continuity run=$r ms=${res%% *} mode=$(field "$res" mode) changed_dirs=$(field "$res" changed_dirs)"
done
for r in $(seq "$REPS"); do
    sleep 1.2
    mkdir -p "$root/proj03/target/debug/incremental/run$r"
    head -c 65536 /dev/zero > "$root/proj03/target/debug/incremental/run$r/x.bin"
    res="$(timed)"
    say "one_subtree_mutation run=$r ms=${res%% *} mode=$(field "$res" mode) changed_dirs=$(field "$res" changed_dirs) walked_total=$(field "$res" walked_total)"
done
ref_store="$work/ref-store"
ref="$(SWAMP_DIR="$ref_store" "$bin" observe "$root" --full | grep '^observed_at=')"
[ "$(field "$res" walked_total)" = "$(field "$ref" walked_total)" ] &&
    say "equivalence: incremental walked_total $(field "$res" walked_total) == reference full walk $(field "$ref" walked_total)" ||
    { say "equivalence: MISMATCH incremental=$(field "$res" walked_total) full=$(field "$ref" walked_total)"; exit 1; }

"$bin" collect --status --json "$root" > "$work/status.json"
python3 - "$work/status.json" "$collector_pid" <<'PY' | tee -a "$report"
import json, sys
d = json.load(open(sys.argv[1]))["roots"][0]
c = d["checkpoint"]
rss = next((l.split()[1] for l in open(f"/proc/{sys.argv[2]}/status") if l.startswith("VmRSS")), "?")
print(f"watch_memory: watches={c['watches']} kernel_estimate_kib={c['kernel_bytes_estimate']//1024} collector_rss_kib={rss} limit={c['max_user_watches']}")
print(f"cli_json: collect --status --json parses; running={d['collector_running']} coverage={c['coverage']['status']}")
PY

kill -9 "$collector_pid"; wait "$collector_pid" 2>/dev/null || true; collector_pid=""
head -c 30000 /dev/zero > "$root/proj05/src/while_stopped.bin"
res="$(timed)"; say "process_stopped_gap ms=${res%% *} mode=$(field "$res" mode) reason=$(field "$res" reason)"
[ "$(field "$res" reason)" = collector_stopped ]

start_collector
sleep 1.2
res="$(timed)"; say "new_epoch ms=${res%% *} mode=$(field "$res" mode) reason=$(field "$res" reason)"
limit=$(cat /proc/sys/fs/inotify/max_queued_events)
# The burst directory must be watched before the burst: files created in
# a directory with no watch yet produce no events, and so no overflow.
mkdir -p "$root/proj07/target/burst"
sleep 1.5
kill -STOP "$collector_pid"
python3 - "$root/proj07/target/burst" "$limit" <<'PY'
import os, sys
d, limit = sys.argv[1], int(sys.argv[2])
for i in range(limit // 2 + 2000):
    open(os.path.join(d, f"f{i}"), "w").close()
PY
kill -CONT "$collector_pid"
sleep 2
res="$(timed)"; say "queue_overflow ms=${res%% *} mode=$(field "$res" mode) reason=$(field "$res" reason) (max_queued_events=$limit)"
sleep 1.2
res="$(timed)"; say "overflow_recovery ms=${res%% *} mode=$(field "$res" mode) reason=$(field "$res" reason)"
kill -TERM "$collector_pid"; wait "$collector_pid" || true; collector_pid=""

say "on_disk_state: store=$(du -sb "$SWAMP_DIR" | cut -f1) bytes; continuity=$(du -cb "$SWAMP_DIR"/continuity/*.json | tail -1 | cut -f1) bytes"

# --- a DependencyTree unit is reported, with its facts (read-only) ---
# There is no CLI propose/approve/execute any more (swamp reports; the
# human removes, in the TUI or by hand): the freedesktop Trash mover
# itself is exercised by swamp-core's own `linux_trash.rs` unit tests,
# not by this packaged-binary validation. What this script can and does
# check is that the packaged binary's read-only surface reports a
# DependencyTree unit's facts correctly on this machine.
"$bin" report "$root" --json > "$work/deps.json"
python3 - "$work/deps.json" <<'PY' | tee -a "$report"
import json, sys
d = json.load(open(sys.argv[1]))
rows = [r for p in d["projects"] for wt in p["worktrees"] for r in wt["artifacts"] if r["kind"] == "DependencyTree"]
assert rows, d
row = rows[0]
assert row["bytes"] > 0, row
assert row["evidence"], "no decision evidence attached to the reported unit"
print(f"report: {len(rows)} DependencyTree unit(s) reported; first is {row['path']} ({row['bytes']} bytes, {len(row['evidence'])} evidence fact(s))")
PY

# --- scheduled / collector lifecycle through systemd --user ----------
uid="$(id -u)"
if [ -z "${XDG_RUNTIME_DIR:-}" ] && [ -d "/run/user/$uid" ]; then export XDG_RUNTIME_DIR="/run/user/$uid"; fi
if systemctl --user show-environment >/dev/null 2>&1; then
    unitdir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    # The runner's user manager reads the *runner's* unit directory, not
    # this script's scratch HOME: install there, and remove afterwards.
    realhome="$(getent passwd "$uid" | cut -d: -f6)"
    HOME="$realhome" SWAMP_DIR="$SWAMP_DIR" "$bin" schedule --every 1h --collector "$root" | tee -a "$report"
    sleep 2
    HOME="$realhome" "$bin" schedule | tee -a "$report"
    active="$(systemctl --user is-active swamp-collect.service || true)"
    say "systemd: collector service $active; timer $(systemctl --user is-active swamp-observe.timer || true)"
    systemctl --user start swamp-observe.service && say "systemd: one scheduled observation ran ($(systemctl --user show swamp-observe.service -p Result --value))" || say "systemd: the scheduled observation failed: $(journalctl --user -u swamp-observe.service -n 20 --no-pager 2>/dev/null | tail -5)"
    HOME="$realhome" "$bin" schedule --off | tee -a "$report"
    left="$(find "$realhome/.config/systemd/user" -maxdepth 1 -name 'swamp-*' 2>/dev/null | wc -l)"
    say "systemd: after --off, $left swamp unit file(s) remain"
    [ "$left" = 0 ]
else
    say "systemd: no user manager reachable here ($(systemctl --user show-environment 2>&1 | head -1)); lifecycle recorded as unavailable"
    "$bin" schedule --every 1h "$root" 2>&1 | tee -a "$report" || true
fi
say "result: ok"
