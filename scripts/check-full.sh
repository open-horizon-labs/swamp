#!/usr/bin/env bash
# The full tier: everything `scripts/check.sh` runs, then the heavy
# harnesses, each exactly once -- the compile-fail cases (every retired
# rule's shortcut against the production API), the mutation sweep (every
# corpus, re-review and operator mutation applied to a copy of the
# workspace) and the cost test (process-global work counters and a PATH
# spawn shim, so single-threaded and alone). What CI runs
# (`.github/workflows/check-full.yml`).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
target=()
if [ -n "${SWAMP_TARGET_DIR:-}" ]; then
  target=(--target-dir "$SWAMP_TARGET_DIR")
fi
step() { printf '\n== %s (%s)\n' "$1" "$(date +%H:%M:%S)"; }

"$here/check.sh"

step compile-fail
# The compile-fail cases compare rustc's exact diagnostic text
# (trybuild's `.stderr` snapshots), which drifts between rustc versions.
# Pin this step to the toolchain in rust-toolchain.toml explicitly
# (rather than trusting the ambient/override resolution) so a caller who
# has since `rustup default`-ed something else still gets the toolchain
# the snapshots were generated against, matching CI's pinned runner.
pinned=$(awk -F'"' '/^channel/{print $2}' "$here/../rust-toolchain.toml")
rustup run "$pinned" cargo test -p swamp-source-audit --locked ${target[@]+"${target[@]}"} --test compile_fail -- --ignored

step mutation-sweep
cargo test -p swamp-source-audit --locked ${target[@]+"${target[@]}"} --test mutation_sweep -- --ignored

step cost
cargo test -p swamp-core --locked ${target[@]+"${target[@]}"} \
  --test reviewer_cost_measurement_stack3 \
  -- --test-threads=1
step done
