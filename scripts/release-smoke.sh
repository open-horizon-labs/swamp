#!/usr/bin/env bash
# Smoke-tests a packaged release archive the way a user installs it (#88,
# #89): verify the checksum, extract, and run the extracted binary --
# never the one in target/.
#
#   scripts/release-smoke.sh <archive.tar.gz> <expected-version> [<report-dir>]
#
# Checks: the checksum; the version string; --help; the packaged skill;
# an explicit-root `report --json`; `observe` into a scratch store and a
# second `report --json` reading that history back; TUI startup and a
# clean quit under a pseudo-terminal; and, on Linux, linkage (`ldd`) and
# the newest glibc symbol version the binary needs (`objdump -T`). With a
# report dir, the linkage and the timings are written there as the CI
# artifact the docs quote.
set -euo pipefail

archive="${1:?archive}"
version="${2:?expected version}"
report="${3:-}"

here="$(pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sha256_check() {
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$(dirname "$1")" && sha256sum -c "$(basename "$1").sha256")
    else
        (cd "$(dirname "$1")" && shasum -a 256 -c "$(basename "$1").sha256")
    fi
}

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

sha256_check "$archive"
t0=$(now_ms)
tar -C "$work" -xzf "$archive"
dir="$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -1)"
bin="$dir/swamp"
[ -x "$bin" ] || { echo "no executable swamp in $archive" >&2; exit 1; }
test -f "$dir/skills/swamp/SKILL.md" || { echo "skill missing from $archive" >&2; exit 1; }
test -f "$dir/README.md"

got="$("$bin" --version)"
[ "$got" = "swamp ${version}" ] || { echo "version: got '$got', expected 'swamp ${version}'" >&2; exit 1; }
"$bin" --help | head -3

# A generated fixture: a Cargo project with a build directory and a
# node project, nothing from the machine running this.
fx="$work/fixture"
mkdir -p "$fx/proj/src" "$fx/proj/target/debug/deps" "$fx/web/node_modules/pkg"
printf '[package]\nname = "proj"\nversion = "0.1.0"\n' > "$fx/proj/Cargo.toml"
: > "$fx/proj/src/lib.rs"
head -c 200000 /dev/zero > "$fx/proj/target/debug/deps/libproj.rlib"
printf '{"name":"web"}\n' > "$fx/web/package.json"
head -c 50000 /dev/zero > "$fx/web/node_modules/pkg/index.js"
(cd "$fx/proj" && git init -q . 2>/dev/null || true)

store="$work/store"
export SWAMP_DIR="$store" HOME="$work/home" SWAMP_TEST_MODE=1
mkdir -p "$HOME"
# `report` is a pure read (R12): a scope with no prior `observe` has
# nothing to read, and exits 2 with a `no_observation` JSON error rather
# than an empty-but-successful report.
! "$bin" report "$fx" --json > "$work/r0.json"
grep -q '"error": *"no_observation"' "$work/r0.json"
t1=$(now_ms)
"$bin" observe "$fx" | tee "$work/observe1.txt"
t2=$(now_ms)
"$bin" observe "$fx" | tee "$work/observe2.txt"
t3=$(now_ms)
"$bin" report "$fx" --json > "$work/r1.json"
grep -q '"projects"' "$work/r1.json"

# TUI: starts, draws, quits on `q`, under a pseudo-terminal.
tui="skipped (no script(1))"
if command -v script >/dev/null 2>&1; then
    if [ "$(uname -s)" = Linux ]; then
        (sleep 3; printf 'q') | timeout 30 script -qec "$bin ui $fx" /dev/null >/dev/null
    else
        (sleep 3; printf 'q') | script -q /dev/null "$bin" ui "$fx" >/dev/null
    fi
    tui="started and quit cleanly"
fi
echo "tui: $tui"

linkage=""
glibc=""
if [ "$(uname -s)" = Linux ]; then
    linkage="$(ldd "$bin" || true)"
    glibc="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 || true)"
    echo "ldd:"; echo "$linkage"
    echo "newest glibc symbol version required: ${glibc:-none}"
fi

if [ -n "$report" ]; then
    mkdir -p "$report"
    {
        echo "archive: $(basename "$archive")"
        echo "version: $got"
        echo "os: $(uname -srm)"
        [ -f /etc/os-release ] && . /etc/os-release && echo "distribution: ${PRETTY_NAME:-unknown}"
        command -v ldd >/dev/null 2>&1 && echo "libc: $(ldd --version 2>&1 | head -1)"
        echo "install_to_first_report_ms: $((t1 - t0))  (checksum-verified extract -> report --json)"
        echo "first_observe_ms: $((t2 - t1))  ($(cut -d' ' -f5- "$work/observe1.txt" | head -1))"
        echo "second_observe_ms: $((t3 - t2))  ($(cut -d' ' -f5- "$work/observe2.txt" | head -1))"
        echo "tui: $tui"
        echo "newest_glibc_symbol: ${glibc:-n/a}"
        echo "ldd:"
        echo "$linkage" | sed 's/^/  /'
    } > "$report/$(basename "$archive" .tar.gz).smoke.txt"
    cat "$report/$(basename "$archive" .tar.gz).smoke.txt"
fi
cd "$here"
echo "smoke ok: $archive"
