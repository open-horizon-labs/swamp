#!/usr/bin/env bash
# Behavioural test for scripts/verify-gates.sh against a fake `gh`.
# Each case scripts what `gh run list` returns per workflow on successive
# polls, which runs have failed jobs, and asserts the verdict, what was
# dispatched, and (for fail-fast) that it did not just time out.
# Portable to bash 3.2 (macOS runners): no associative arrays.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The fake gh. `run list --workflow WF` prints the next line of polls.WF
# (repeating the last one): rows separated by `;`, fields `id,status,conclusion`.
# `run view ID` prints the failed job names in jobs.ID. `workflow run WF`
# records WF in `dispatched`.
cat >"$work/gh" <<'EOF'
#!/usr/bin/env bash
w=$(dirname "$0")
case "$1 $2" in
  "run list")
    wf=""
    while [ $# -gt 0 ]; do [ "$1" = "--workflow" ] && wf=$2; shift; done
    n=$(cat "$w/n.$wf" 2>/dev/null || echo 0)
    total=$(wc -l <"$w/polls.$wf" 2>/dev/null || echo 0)
    [ "$total" -eq 0 ] && exit 0
    line=$(sed -n "$((n + 1 > total ? total : n + 1))p" "$w/polls.$wf")
    echo $((n + 1)) >"$w/n.$wf"
    [ -z "$line" ] && exit 0
    printf '%s\n' "$line" | tr ';' '\n' | awk -F, '{print $1"\t"$2"\t"$3"\thttps://x/"$1}'
    ;;
  "run view") cat "$w/jobs.$3" 2>/dev/null || true ;;
  "workflow run") echo "$3" >>"$w/dispatched" ;;
esac
EOF
chmod +x "$work/gh"

reset() { rm -f "$work"/n.* "$work"/polls.* "$work"/jobs.* "$work"/dispatched; }
# polls <workflow> <poll>...
polls() {
  local wf=$1
  shift
  : >"$work/polls.$wf"
  for p in "$@"; do printf '%s\n' "$p" >>"$work/polls.$wf"; done
}
failed_jobs() {
  local id=$1
  shift
  printf '%s\n' "$@" >"$work/jobs.$id"
}

# verdict <name> <want exit> <want dispatched (space-joined, or "")> [want output substring]
verdict() {
  local name=$1 want=$2 dispatch=$3 sub=${4:-}
  local got=0
  GH="$work/gh" VERIFY_POLL=0 VERIFY_TIMEOUT=3 \
    "$here/verify-gates.sh" abc123 v1.2.3 o/r >"$work/out" 2>&1 || got=$?
  local d=""
  [ -f "$work/dispatched" ] && d=$(tr '\n' ' ' <"$work/dispatched" | sed 's/ $//')
  if [ "$got" -ne "$want" ] || [ "$d" != "$dispatch" ] ||
    { [ -n "$sub" ] && ! grep -q -- "$sub" "$work/out"; }; then
    echo "FAIL $name: exit=$got (want $want) dispatched='$d' (want '$dispatch') want-output='$sub'"
    cat "$work/out"
    exit 1
  fi
  echo "ok   $name"
}

reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,completed,success"
verdict "both gates met releases, nothing dispatched" 0 ""

reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,completed,failure"
verdict "a failed full tier blocks the release" 1 "" "did not pass"

reset
polls ci.yml "1,completed,failure"
polls check-full.yml "2,completed,success"
verdict "a failed ci.yml blocks the release even though the full tier passed" 1 "" "did not pass"

reset
polls ci.yml "1,completed,timed_out"
polls check-full.yml "2,completed,success"
verdict "a timed-out ci.yml blocks the release" 1 ""

reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,in_progress,-" "2,in_progress,-" "2,completed,success"
verdict "waits out an in-progress run, then passes" 0 ""

reset
polls ci.yml "1,queued,-" "1,in_progress,-" "1,completed,success"
polls check-full.yml "2,completed,success"
verdict "waits for ci.yml too" 0 ""

reset
polls check-full.yml "2,completed,success"
verdict "no ci.yml run and none appears: dispatched once, then times out" 1 "ci.yml" "timed out"

reset
polls check-full.yml "2,completed,success"
polls ci.yml "" "" "3,queued,-" "3,completed,success"
verdict "no ci.yml run: dispatch it, wait for it, pass" 0 "ci.yml"

reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,completed,cancelled" "4,completed,success"
verdict "only a cancelled full-tier run: dispatch one" 0 "check-full.yml"

reset
polls ci.yml "" "5,completed,success"
polls check-full.yml "" "6,completed,success"
verdict "no runs at all: both dispatched" 0 "ci.yml check-full.yml"

# Fail fast: a job has already failed in the only active run.
reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,in_progress,-"
failed_jobs 2 "Linux x86_64 (check-full)"
verdict "fail fast: a failed job in the only active run fails the gate now" 1 "" "still going"

reset
polls ci.yml "1,in_progress,-"
polls check-full.yml "2,completed,success"
failed_jobs 1 "macOS arm64"
verdict "fail fast applies to ci.yml too" 1 "" "still going"

# Fail fast must not fail early when another active run could still pass.
reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,in_progress,-;3,in_progress,-" "2,in_progress,-;3,in_progress,-" "3,completed,success"
failed_jobs 2 "Linux x86_64 (check-full)"
verdict "one active run failed but another has not: keep waiting" 0 ""

reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,in_progress,-" "2,completed,success"
verdict "an active run with no failed job keeps waiting" 0 ""

reset
polls ci.yml "1,completed,success"
polls check-full.yml "2,in_progress,-"
verdict "a run that never finishes times out" 1 "" "timed out"

echo "all verify-gates cases passed"
