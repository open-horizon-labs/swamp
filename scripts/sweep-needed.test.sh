#!/usr/bin/env bash
# Behavioural test for scripts/sweep-needed.sh.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
expect() {
  local name=$1 want=$2 got
  shift 2
  got=$("$here/sweep-needed.sh" "$@" 2>/dev/null)
  if [ "$got" != "$want" ]; then echo "FAIL $name: got '$got', want '$want'"; exit 1; fi
  echo "ok   $name"
}
expect "the sweep already passed for this clippy and these inputs: skip" skip true
expect "no passing record (inputs or clippy changed, or cache evicted): run" run false
expect "an empty cache state runs" run ""
expect "no argument runs" run
echo "all sweep-needed cases passed"
