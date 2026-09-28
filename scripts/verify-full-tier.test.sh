#!/usr/bin/env bash
# Behavioural test for scripts/verify-full-tier.sh against a fake `gh`.
# Each case scripts what `gh run list` returns on successive polls and
# asserts the verdict, and whether a run was dispatched.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The fake gh: `run list` prints the next line of $work/polls (repeating
# the last), one poll per line with rows separated by `;` and fields by `,`.
cat >"$work/gh" <<'EOF'
#!/usr/bin/env bash
w=$(dirname "$0")
case "$1 $2" in
  "run list")
    n=$(cat "$w/n" 2>/dev/null || echo 0)
    total=$(wc -l <"$w/polls")
    line=$(sed -n "$((n + 1 > total ? total : n + 1))p" "$w/polls")
    echo $((n + 1)) >"$w/n"
    [ -z "$line" ] && exit 0
    printf '%s\n' "$line" | tr ';' '\n' | awk -F, '{print $1"\t"$2"\t"$3"\thttps://x/"$1}'
    ;;
  "workflow run") echo dispatched >>"$w/dispatched" ;;
esac
EOF
chmod +x "$work/gh"

# case <name> <expected exit> <expect dispatch yes|no> <polls...>
case_() {
  local name=$1 want=$2 dispatch=$3
  shift 3
  rm -f "$work/n" "$work/dispatched"
  : >"$work/polls"
  for p in "$@"; do printf '%s\n' "$p" >>"$work/polls"; done
  local got=0
  GH="$work/gh" VERIFY_POLL=0 VERIFY_TIMEOUT=5 \
    "$here/verify-full-tier.sh" abc123 v1.2.3 o/r >"$work/out" 2>&1 || got=$?
  local d=no
  [ -f "$work/dispatched" ] && d=yes
  if [ "$got" -ne "$want" ] || [ "$d" != "$dispatch" ]; then
    echo "FAIL $name: exit=$got (want $want) dispatched=$d (want $dispatch)"
    cat "$work/out"
    exit 1
  fi
  echo "ok   $name"
}

case_ "a passed run releases, nothing dispatched" 0 no "1,completed,success"
case_ "waits out an in-progress run, then passes" 0 no \
  "1,in_progress,-" "1,in_progress,-" "1,completed,success"
case_ "a failed run blocks the release and is never re-run over" 1 no "1,completed,failure"
case_ "a timed-out run blocks the release" 1 no "1,completed,timed_out"
case_ "a failure next to an active run keeps waiting, then fails" 1 no \
  "1,completed,failure;2,in_progress,-" "1,completed,failure"
case_ "no run: dispatch one and wait for it" 0 yes "" "" "2,queued,-" "2,completed,success"
case_ "only a cancelled run: dispatch one" 0 yes "1,completed,cancelled" "2,completed,success"
case_ "a run that never finishes times out" 1 no "1,in_progress,-"
echo "all verify-full-tier cases passed"
