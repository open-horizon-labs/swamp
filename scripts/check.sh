#!/usr/bin/env bash
# The fast local tier (target: under 8 minutes on a warm target dir).
# Every step runs once. The heavy harnesses -- the mutation sweep, the
# compile-fail cases and the cost test -- are `scripts/check-full.sh`'s:
# the first two are `#[ignore]`d so `cargo test --workspace` skips them,
# and the cost test (a reviewer file, which cannot carry `#[ignore]`) is
# skipped by name here and run once, single-threaded, there.
set -euo pipefail
# `SWAMP_TARGET_DIR` (optional) points every cargo run below at a shared
# target directory -- e.g. one checkout's `target/` for several worktrees.
target=()
if [ -n "${SWAMP_TARGET_DIR:-}" ]; then
  target=(--target-dir "$SWAMP_TARGET_DIR")
fi
step() { printf '\n== %s (%s)\n' "$1" "$(date +%H:%M:%S)"; }

step fmt
cargo fmt --all -- --check

# Once, over every target: `--all-targets` includes each library and
# binary in its non-test configuration, which is where the crate roots
# deny the capability gate's lints, so a separate `--lib --bins` pass
# would repeat work already done.
step clippy
cargo clippy --workspace --all-targets --locked ${target[@]+"${target[@]}"} -- -D warnings

step audits
cargo run -q --locked ${target[@]+"${target[@]}"} -p swamp-source-audit

# The shipped build graph must not contain swamp-core's `testing`
# feature (test-fixture API). Only `[dev-dependencies]` enable it, which
# resolver 2 never unifies into a non-test build; this proves it for the
# binary the release workflow packages (`crates/core/src/lib.rs`).
step release-graph
if cargo tree -p swamp -e normal,build,features --prefix none --locked |
  grep -q 'swamp-core feature "testing"'; then
  echo 'release graph: the swamp binary enables swamp-core/testing (test-fixture API)' >&2
  exit 1
fi

# Unit and integration tests, once. The cost test is left to
# check-full.sh: it measures process-global counters and must run alone.
step tests
cargo test --workspace --locked ${target[@]+"${target[@]}"} -- \
  --skip unchanged_observations_spaced_past_the_toosoon_floor

# Named so a rename cannot silently drop them (the runtime halves of the
# review guardrails). Test targets are discovered from these files, and
# the run above already built and ran them; a `cargo test -p <crate>
# --no-run` here would re-resolve features for one package and rebuild
# half the graph for nothing. (`guardrail_metadata` separately proves
# every guardrail's runtime tests exist, run and assert.)
step named-targets
for t in \
  core/tests/reviewer_counterexamples_stack2 \
  core/tests/reviewer_counterexamples_stack3 \
  core/tests/reviewer_counterexamples_stack4 \
  core/tests/reviewer_cost_measurement_stack3 \
  core/tests/coverage_changes_are_not_storage_changes \
  core/tests/explicit_root_scope_exclusions \
  core/tests/nested_artifact_evidence_is_delivered \
  core/tests/upstream_citations_are_checked \
  core/tests/shared_history_ownership \
  core/tests/scope_unique_accounting \
  core/tests/store_contents_are_allowlisted \
  core/tests/incremental_external_and_agent_measurement \
  core/tests/agent_matrix_matches_docs \
  core/tests/agent_storage_validation \
  core/tests/agent_container_seams \
  core/tests/unit_root_event_cursors \
  core/tests/fsevents_incremental \
  core/tests/build_adapter_contract \
  core/tests/build_adapter_cost \
  core/tests/build_adapter_history \
  core/tests/build_store_join \
  tui/tests/scope_preserving_refresh \
  tui/tests/reviewer_counterexamples_stack2_tui \
  source-audit/tests/compile_fail \
  source-audit/tests/mutation_sweep; do
  test -f "crates/$t.rs" || { echo "named test target crates/$t.rs is gone" >&2; exit 1; }
done

step greps
# These checks intentionally fail obvious safety regressions in source
# review: a raw recursive delete, and verdict vocabulary the tool never
# applies to a path ("safe", "unused", "stale" are for the human to
# conclude, never for the tool to assert).
#
# `grep`, not `rg`: this has to run wherever CI or a contributor runs it,
# and ripgrep is not a dependency of this repo.
#
# Comment lines are excluded because the rule is written down in the
# source that enforces it: render.rs and filter.rs each explain the
# verdict-vocabulary ban using the very words it bans, so matching
# comments made this check impossible to pass. A trailing comment on a
# line of code is still reported -- only a line that is nothing but a
# comment is skipped.
violations=$(
  grep -rnE 'rm[[:space:]]+-rf|std::fs::remove_dir_all|"safe"|"unused"|"stale"' \
    crates/core/src crates/cli/src |
    awk '{ code = $0; sub(/^[^:]*:[0-9]+:/, "", code); if (code !~ /^[[:space:]]*\/\//) print }'
) || true
if [ -n "$violations" ]; then
  echo "$violations" >&2
  echo 'source audit: forbidden destructive/verdict shortcut found' >&2
  exit 1
fi

# Protection state is a Result and no caller may discard it: the AST
# audit checks the call sites it can see, this catches the shape
# anywhere it appears (.oh/guardrails/protection-fails-closed.md).
protect_violations=$(
  grep -rn 'unwrap_or_default()' crates/core/src crates/cli/src crates/tui/src |
    grep 'protect' |
    awk '{ code = $0; sub(/^[^:]*:[0-9]+:/, "", code); if (code !~ /^[[:space:]]*\/\//) print }'
) || true
if [ -n "$protect_violations" ]; then
  echo "$protect_violations" >&2
  echo 'source audit: protection state must fail closed, never unwrap_or_default()' >&2
  exit 1
fi

# JSON is an output format and a format for small control files, never a
# data store (.oh/guardrails/json-persistence-is-allowlisted.md). Belt
# and braces over the AST audit: a serializer call in a file that is not
# an allow-listed writer, and is not printing, fails here.
json_writer_files='fs_gate/destroy.rs|actions.rs|continuity.rs|collect.rs|grants.rs|ledger.rs|schedule.rs|scope.rs|agents/mod.rs|cargo_cleanup.rs|growth.rs|report.rs|assoc_store.rs|agent_json.rs|app.rs|main.rs|render.rs|docker.rs|github.rs|store.rs|recovery.rs|evidence.rs|entities.rs'
json_violations=$(
  grep -rnE 'serde_json::to_(vec|string|writer)' crates/core/src crates/cli/src crates/tui/src |
    grep -vE "/($json_writer_files):" |
    awk '{ code = $0; sub(/^[^:]*:[0-9]+:/, "", code); if (code !~ /^[[:space:]]*\/\//) print }'
) || true
if [ -n "$json_violations" ]; then
  echo "$json_violations" >&2
  echo 'source audit: JSON serialization outside the allow-listed writers' >&2
  exit 1
fi

# Every production subprocess starts in the gate (`fs_gate::spawn::run`,
# which counts it); `std::process` anywhere else in core, cli or tui is a
# gate path the audit rejects and clippy denies. What neither sees is
# test code: a test fixture that runs `git` itself must still count the
# spawn, or a cost test reading the counters would miss it. So every
# `Command::new` outside the gate must be preceded by `record_spawn()`.
missing_spawn_counts=$(
  grep -rn 'Command::new(' crates/core/src crates/cli/src crates/tui/src |
    grep -v '^crates/core/src/fs_gate/' |
    awk '{ code = $0; sub(/^[^:]*:[0-9]+:/, "", code); if (code !~ /^[[:space:]]*\/\//) print }' |
    while IFS= read -r hit; do
      file=${hit%%:*}
      rest=${hit#*:}
      line=${rest%%:*}
      if ! sed -n "$((line - 2)),$((line - 1))p" "$file" | grep -q 'record_spawn()'; then
        echo "$hit"
      fi
    done
) || true
if [ -n "$missing_spawn_counts" ]; then
  echo "$missing_spawn_counts" >&2
  echo 'source audit: a Command::new outside crates/core/src/fs_gate is not preceded by work_counters::record_spawn()' >&2
  exit 1
fi

test -x scripts/check.sh
test -x scripts/check-full.sh
step done
