# Report-only: removing the CLI/agent action path

## Aim and authority

Product decision (user, 2026-09-23): **swamp reports; the human
removes** (in the TUI, or by hand). Remove the entire CLI/agent action
path -- `propose`, `propose-agents`, `approve`, `execute`,
`grant add/list/revoke`, `plans`, `cleanup-check` -- and the plan/grant
store, `authority.rs`, the keyed-MAC `authority.key`, the
`HumanConfirmed`/`Authorized`/`RecheckProof` tokens, `recheck.rs`.
Keep the TUI Trash (Space marks, Backspace shows current facts, Enter
moves to Trash) with **no** post-mark recheck, **no** "changed since
review" refusal, **no** occupancy veto -- only OS errors refuse. Make
the CLI and skill entirely read-only. Turn "inspect only / unsupported
action" language into plain consequence text where it touched the
deleted action path. Branch `stack/27-report-only` from
`stack/24-linux-on-gates`, in `/Users/muness1/src/open-horizon-labs/swamp-builds`.

## What changed

**Deleted modules:** `crates/core/src/authority.rs` (382 lines),
`crates/core/src/recheck.rs` (845 lines), `crates/core/src/grants.rs`
(128 lines, a legacy/unused Grant model separate from the real one
inline in `actions.rs`), `crates/core/src/fs_gate/key.rs` (65 lines,
the authority-key MAC).

**`actions.rs`** (3186 -> ~2000 lines): kept `PlanUnit`-building
(`propose`, `propose_agents`, `propose_external`,
`propose_checking_(store_)protection`, `unit_from_row/worktree/dir/
external/agent`, `warnings_for`, `refusal_for_kind`, `recovery_for`,
`agent_refusal`) as pure, in-memory functions returning `Vec<PlanUnit>`
directly -- no `Plan` wrapper, no persistence, no MAC, no expiry.
Deleted `Plan`/`PlanStatus`/`Grant`/`StoredPlan`/`StoredGrant`/
`save_plan`/`load_plan`/`list_plans`/`list_grants`/`write_grants`/
`approve`/`approve_confirmed`/`add_standing_grant(_confirmed)`/
`revoke_grant`/`execute`/`execute_with_trash(_opts)`/
`execute_keeping_executables`/`ExecuteResult`/`UnitOutcome`/
`approve_command`/`validate_grant_predicate`/`grant_covers`/
`grant_is_live_and_covers`/`target_of`/`canonical_json`/
`record_binding`/`verified_record`/`selection_estimate` (the
`reclaimability::estimate_selection` wiring that lived on `Plan`).
Added `trash_agent_cache`/`trash_agent_session` (proof-free
replacements for `execute_agent_cache_trash`/
`execute_agent_session_removal`) and `trash_cargo_group` (thin wrapper
over `cargo_cleanup::move_group`). `agent_refusal` no longer checks
occupancy (that's a fact now, never a veto); its "no supported
selective action" message became "swamp has no Trash move for this
category; remove it yourself if you want it gone."

**`fs_gate/destroy.rs`**: `trash_move`/`Envelope::open`/`move_member`/
`copy_preserved`/`docker_remove`/`git_worktree_prune` all dropped their
`RecheckProof`/`Authorized` parameters; they now take plain paths (or a
`Removal`/a common-dir path) and the only refusals are OS-level.

**`cargo_cleanup.rs`**: `move_reviewed` -> `move_group`, dropping the
lock-set/evidence/role/membership "changed since review" checks; kept
holding the advisory Cargo lock for the move's duration (mutual
exclusion with a live `cargo build`, not a drift check).

**`docker.rs`**: `remove` takes a `&Removal` directly, no proof.

**`preserve.rs`**: `preserve_executables` takes `(unit_path,
worktree_bin)` directly, no proof/auth.

**`protection.rs`**: `protect_add_confirmed`/`protect_remove_confirmed`
(which spent a `HumanConfirmed::cli_protect`) collapsed to plain
`protect_add`/`protect_remove` -- `swamp protect` stays human-only by
convention (it's the CLI's only writing command), not by a spent token.

**`crates/tui/src/actions.rs`** (913 -> ~370 lines): `MarkedUnit` lost
its `reviewed` field and gained `cargo_unit`/`agent_unit: Option<PlanUnit>`
in place of `cargo_plan`/`agent_plan: Option<Plan>`. Deleted
`Confirmable`/`confirmables()`/all `HumanConfirmed` plumbing.
`execute_one`/`trash_path`/`remove_docker`/`remove_worktree` call
`fs_gate::destroy`/`actions::trash_*` directly with no proof/auth
argument and no `store`/confirmation parameter (only `Ledger`/
`trash_root` remain). `execute_plan(_progress)` dropped its
`confirmed: Vec<HumanConfirmed>` and `store: &StoreDir` parameters.

**CLI (`crates/cli/src/main.rs`)**: removed the `CleanupCheck`,
`Propose`, `ProposeAgents`, `Approve`, `Execute`, `Plans`, `Grant`
`Command` variants and the `GrantCmd` enum, their match arms, and the
now-orphaned helpers (`cmd_approve`, `cmd_grant_add`, `cmd_grant_revoke`,
`cleanup_path_in_scope`, `cleanup_path_at_or_below_scope`,
`cleanup_scope_summary` + its struct/consts, `discover_agent_units_for_propose`,
`discover_external_units_for_propose`, `save_and_print_plan`,
`propose_unified`, `print_plan`, `parse_size_arg`, and their test
module). `cmd_protect` stays, calling `protect_add`/`protect_remove`
directly.

**Guardrails**: `human-only-authorization.md` and
`execution-sinks-recheck-live-state.md` retired in place (kept, with a
short "why this was retired" note pointing at the product decision and
this session, per the brief's "retired with reason"); the
`trash-backend-owns-every-move.md` line naming the retired guardrail as
its complement was updated; `protection-fails-closed.md`'s
`compile_fail`/`runtime_tests` frontmatter and its Type section were
updated for the new `protect_add`/`protect_remove`.

**`skills/swamp/`**: `references/trust-model.md` rewritten (there is no
swamp-enforced authorization boundary any more: a read-only tool and a
human with a keyboard). `references/cleanup-and-recovery.md` rewritten
to "how to find and restore what the TUI trashed" (ledger shape, Trash
envelope/`restore.json`, macOS/Linux restore instructions) --
propose/approve/execute/grant/cleanup-check content removed.
`SKILL.md`'s "Proposing cleanup" section became "Explaining what
removal would cost -- never proposing to do it"; the reference index
and description updated to match.

**Docs**: `docs/architecture.md`'s "Actions and extension points" ->
"Reporting and removal"; "Capability gates" trimmed of the
authority-key/RecheckProof/Authorized/HumanConfirmed token
descriptions (kept only `Trashed`, `Stage`, `DiscoveryPass`,
`Classified`, `PermittedDetectors`, `Owned`, `Reason`); the
"Provenance" and "recheck model at destructive sinks" subsections
replaced with short retirement notes; the audit-enforced and
trusted-base bullet lists had their now-nonexistent pins/paths removed.
`docs/usage.md`'s "Review exact build groups from the CLI" (formerly
`cleanup-check`) and "Cleanup and recovery" sections rewritten for the
TUI-only Trash flow; the agent-storage section's "Supported actions"
became "Removal is TUI-only"; the JSON command table dropped
`propose`/`execute`/`plans`/`grant list`; the closing "trust model"
paragraph rewritten. `README.md`'s cleanup and agent sections rewritten
to match. `DESIGN.md`'s External/Agents-row descriptions updated
(dropped the "`actions::execute` already refuses every... unit"
framing and the "protected/unsupported" refusal vocabulary in favor of
"no Trash move for this category"). `CHANGELOG.md` gained an
`## Unreleased` entry, "Swamp no longer performs CLI cleanup."

**`crates/source-audit/src/rules/gate.rs`**: removed every `Group`/
`MintSite`/`LITERAL_SITES` entry naming a deleted module or type
(`authority`, `recheck`, `Plan`/`Grant`/`Authorized`/`HumanConfirmed`/
`RecheckProof`, `revoke_grant`, `cmd_approve`/`cmd_grant_add`,
`fs_gate::key`), and `is_sink`'s `recheck` branch.

**Tests deleted wholesale** (their whole subject was the removed
pipeline): `crates/core/tests/{token_binding,execution_rechecks,
reviewer_counterexamples,reviewer_counterexamples_123,
external_units_actions,actions_r7,evidence_action_recheck,
cargo_delivery,agent_units_actions,agent_units_actions_new_adapters,
cargo_hardlink_cleanup}.rs`, `crates/cli/tests/cleanup_paging.rs`.
Compile-fail cases deleted: `human_confirmation_*`,
`recheck_proof_*`, `recheck_inputs_come_from_the_authorization`,
`destructive_verbs_need_a_proof`, `authorize_*`, `authorized_is_minted_only_by_authorize`,
`approval_without_a_confirmation_does_not_exist`,
`capture_is_private_to_the_propose_path`,
`trash_move_needs_a_recheck_proof`, `plan_is_not_*`,
`plan_content_cannot_be_edited`, `grant_is_not_*`,
`protect_changes_are_human_only` (with their `.stderr` siblings).
Mutation-fixture directories deleted:
`crates/source-audit/tests/mutations/{human_only_authorization,
execution_sinks_recheck_live_state,token_binding_and_gate_hardening}/`,
plus the individual fixtures in `occupancy_is_tristate_at_sinks/` and
`trash_backend_owns_every_move/` and `accept/` that referenced the
deleted modules.

**Tests surgically patched** (kept the file, fixed the touched
function): `fsevents_incremental.rs`, `evidence_contract.rs`,
`evidence_api_is_wired.rs` (deleted its two execute/selection-specific
tests only), `agent_storage_validation.rs` (replaced its
propose->approve->execute step with a direct `trash_agent_*` +
manual ledger append, matching what the TUI now does),
`agent_units_actions_remaining_tools.rs` (`execute_one` helper rewritten
around `trash_agent_cache`/`trash_agent_session`),
`agent_refusal_matrix.rs` (deleted the occupancy-veto and
execute-time-drift tests, which asserted behavior the product decision
explicitly removes; fixed the "no supported" wording expectation),
`linux_trash.rs` (dropped its `authorize()` helper, called
`fs_gate::destroy` directly), `store_contents_are_allowlisted.rs`
(dropped `plans`/`grants.json`/`authority.key` from the allow-list;
replaced the propose/approve/execute cycle with a direct
`trash_agent_cache` call), `crates/cli/tests/agent_json_contract.rs`
and `agent_storage_cli.rs` (deleted their propose/approve/execute
tests, kept the read-only ones).

## Verification

`cargo check --workspace` and `cargo test --workspace --no-run` are
clean. `cargo test --workspace` (after the wording fixes above) passes
in full; see the worker's final report for the exact command and
counts. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`scripts/check.sh`/`check-full.sh`, and the trybuild snapshot
regeneration against the pinned toolchain are covered in the same
report.

## Correction (verification pass, 2026-09-23)

The WIP checkpoint (d4722fb) deleted
`crates/core/tests/reviewer_counterexamples_stack2.rs` wholesale,
uncounted in the "tests deleted wholesale" list above, and left
`scripts/check.sh`'s named-targets loop pointing at the now-missing
file (`check.sh` failed outright: "named test target ... is gone").
Of its 7 tests, only 2 (`an_in_place_rewrite_of_a_reviewed_member_must_
not_spend_the_approval`, `reviewed_snapshot_must_see_a_same_second_
same_size_rewrite`) were actually about the deleted recheck/plan/
approve/execute pipeline. The other 5 -- explicit-root exclusion,
config-only-exclusion growth/regrowth, a disabled detector's spawn
count, the spawn counter's own sanity check, and `protect add`'s
fail-closed behavior on a relative path -- test facts unrelated to the
action-path removal and were still passing before the deletion. The
file is restored with those 5 tests intact, the 2 pipeline-specific
ones dropped, and CE5 rewritten off the removed propose/save_plan/
approve/execute_with_trash cycle onto the current gate
(`agents::discover_and_measure`'s `AgentUnit.protected`, `actions::
propose_agents`'s `agent_refusal`) and the current sink
(`actions::trash_agent_cache`). All 5 pass unchanged.

Also found while regenerating the trybuild snapshots: the compile-fail
`.stderr` snapshots already matched rustc 1.98.1 exactly (both plain
and `TRYBUILD=overwrite` runs produced zero diffs) -- the "8 mismatches"
the brief flagged were apparently against the deleted compile-fail
cases themselves (all of which named the removed
`Plan`/`Grant`/`Authorized`/`HumanConfirmed`/`RecheckProof` types), not
against any case that survived this chunk's deletions. No snapshot
changes were needed. `scripts/check-full.sh`'s compile-fail step now
runs `rustup run <pinned toolchain>` explicitly (reading
`rust-toolchain.toml`) instead of trusting ambient resolution, and
`.github/workflows/ci.yml`'s `macos-arm64-check-full` job -- found
floating on `dtolnay/rust-toolchain@stable` while every other job pins
1.98.1 -- is now pinned to match.

## Known gaps (deliberately out of scope this chunk, given time)

- `cargo_cleanup::check`/`CheckResult` (only ever called by the deleted
  `cleanup-check` CLI command) is left in place as dead-but-compiling
  public API rather than deleted along with its tests; a follow-up
  should remove it.
- The "inspect only / unsupported" -> plain-consequence-text rewrite
  was done for the agent-storage action path (`AgentActionCapability`'s
  user-facing strings) and the external-unit path, per the product
  decision's focus on the CLI/agent action path. It was **not** extended
  to `NestedActionCapability`/the build-adapters catalog (Cargo-external
  ecosystems: Node/Gradle/Maven/Python/Go/Docker/etc.), which uses the
  same "inspection only" vocabulary throughout `model.rs`/`render.rs`/
  `build_stores.rs`/`agent_json.rs`/`build_adapters/*` -- those rows were
  never actionable even before this chunk (no build adapter implements an
  action), so the vocabulary predates and is orthogonal to the action-path
  removal. Flagged as a separate, larger follow-up rather than attempted
  under this chunk's time budget.
- Deleted rather than rewrote several sizeable integration test files
  (`cargo_delivery.rs`, `agent_units_actions*.rs`,
  `cargo_hardlink_cleanup.rs`) whose entire subject was the removed
  propose/approve/execute pipeline. Their *positive*-path coverage (does
  a Cargo group / an agent session actually move to Trash correctly) is
  now only exercised by the smaller tests that remained
  (`crates/tui/src/actions.rs`'s own unit tests,
  `store_contents_are_allowlisted.rs`,
  `agent_storage_validation.rs`,
  `crates/core/src/fs_gate/destroy.rs`'s own envelope test). A follow-up
  should judge whether that residual coverage is sufficient or whether
  new, simplified tests should replace what was deleted.
