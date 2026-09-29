#!/usr/bin/env bash
# Whether the mutation sweep (crates/source-audit/tests/mutation_sweep.rs)
# has anything new to say. It tests the audit rules against ~225 known-bad
# snippets, so its verdict only changes when the rules, the lint
# configuration, the gate they are written around, the lockfile or clippy
# change. Prints `run` or `skip`.
#
# usage: sweep-needed.sh <cache-hit>
#
# `check-full.yml` keys a small cache on the clippy version and a hash of
# those inputs and saves it only after the sweep passed, so `cache-hit` is
# true exactly when the sweep already passed for this clippy and these
# inputs: skip. Anything else, including a cache that was evicted or is in
# an unknown state, runs it. There is no other schedule.
set -euo pipefail

hit=${1:-false}
if [ "$hit" = true ]; then
  echo "skip: the sweep already passed for this clippy version and these inputs" >&2
  echo skip
else
  echo "run: the sweep has not passed for this clippy version and these inputs" >&2
  echo run
fi
