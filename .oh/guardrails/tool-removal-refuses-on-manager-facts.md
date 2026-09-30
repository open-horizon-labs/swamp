---
id: tool-removal-refuses-on-manager-facts
severity: hard
statement: "Specific to removal with no undo (tool-managed removal, #177: `mise uninstall`, `mise prune --tools`, `xcrun simctl runtime delete`). There is no Trash to fall back on, so this one action class keeps automated refusals and a review-to-Enter recheck that every Trash move gave up on 2026-09-23. A removal runs only after the manager's own dry run, only through a manager binary resolved from a fixed list of directories with an environment built from nothing, only from the human's Enter on the TUI confirm, and only if a full re-review at Enter finds nothing changed. Every refusal below is fail-closed: a fact that could not be read refuses."
outcome: decision-relevant-storage-evidence
audit: gate_paths_only_inside_gates
runtime_tests:
  - crates/core/tests/tool_removal_adversarial.rs::global_config_version_refused_even_though_mise_dry_run_exits_0
  - crates/core/tests/tool_removal_adversarial.rs::source_null_from_cwd_is_not_no_consumer
  - crates/core/tests/tool_removal_adversarial.rs::prune_runs_only_as_prune_tools
  - crates/core/tests/tool_removal_adversarial.rs::a_dry_run_without_its_flag_is_not_a_read
  - crates/core/tests/tool_removal_adversarial.rs::simctl_all_and_set_flags_are_never_an_operand
  - crates/core/tests/tool_removal_adversarial.rs::a_booted_simulator_refuses_its_runtime
  - crates/core/tests/tool_removal_adversarial.rs::the_command_run_is_the_command_shown
  - crates/core/tests/tool_removal_adversarial.rs::a_preview_gone_out_of_date_refuses_at_enter
  - crates/core/tests/tool_removal_adversarial.rs::an_open_file_check_that_could_not_finish_blocks
  - crates/core/tests/tool_removal_adversarial.rs::garbage_huge_and_markerless_dry_runs_refuse
  - crates/core/tests/tool_removal_adversarial.rs::the_child_environment_is_built_from_nothing
  - crates/core/tests/tool_removal_adversarial.rs::a_mise_earlier_on_path_is_not_used
  - crates/core/tests/tool_removal_adversarial.rs::no_real_manager_is_reachable_from_a_test
  - crates/core/tests/tool_removal_adversarial.rs::no_cli_path_reaches_tool_removal
  - crates/tui/tests/tool_removal_sheet.rs::enter_on_a_confirm_that_does_not_fit_runs_nothing
---

## Rationale

On 2026-09-23 swamp stopped vetoing the human: a Trash move runs on
Enter with the facts shown and no recheck (`execution-sinks-recheck-live-state`,
`occupancy-is-tristate-at-sinks` and `human-only-authorization` are
retired). That is safe because a Trash move comes back. A manager's own
removal does not: `mise uninstall` deletes the install, `simctl runtime
delete` deletes the disk image. The maintainer decided (#177, 2026-09-30)
to build it anyway, human-confirmed in the TUI only, with the refusals
below. This guardrail applies to that action class and nothing else.

The managers do not protect the human themselves (verified read-only
2026-09-30, mise 2026.9.15, xcrun 72):

- `mise uninstall --dry-run node@24.14.1` exits 0 while the global config
  requests that version.
- Bare `mise prune` also prunes tracked config links.
- `mise ls --json` answers relative to its cwd; `source: null` there does
  not mean nothing requests the version.
- `simctl runtime delete` shuts booted simulators down and deletes
  anyway, and accepts the alias `all` and set flags in the operand slot.

## The refusals (each one, and why)

1. **Manager not found, or not trusted.** The binary is looked up only
   in a fixed list (`/opt/homebrew/bin`, `/usr/local/bin`,
   `~/.local/bin`, `~/.cargo/bin` for mise; `/usr/bin/xcrun`), never on
   `PATH`; the canonical file and its directory must be owned by the user
   or root and writable by nobody else. A shim earlier on `PATH` is never
   run.
2. **A config requests the version** (`source` in `mise ls`): the global
   config is named as such (`mise unuse -g <tool>` is the next step).
3. **mise reports the version active, or it is a symlink install, or it
   lives outside mise's default data dir** (swamp does not pass
   `MISE_DATA_DIR`).
4. **mise's prune does not list the version.** Only prune knows the
   configs mise tracks, so a per-version removal is offered only inside
   prune's own set.
5. **The dry run did not run cleanly**: a non-zero exit, a timeout,
   output over 1 MiB, an `ERROR` line, a line swamp does not recognize,
   a missing dry-run marker (`✓ uninstalled (dry-run)`, `[dryrun] ✓
   done`, simctl's single `Would delete ... <UUID>` line), a
   configuration-links line, a path outside mise's own directories, or
   not exactly the target.
6. **A simulator on the runtime is booted**; the runtime is not
   `deletable`, or its state is not `Ready`/`Unusable`. Unbooted devices
   do not refuse: the confirm names how many and which (maintainer
   decision 5).
7. **Files are held open**, or the open-file check could not finish
   (`Unknown` refuses: maintainer decision 4). CoreSimulator's own
   `SimLaunchHost`, which simctl stops before deleting, is the one holder
   that does not count.
8. **Anything changed between the confirm and Enter**: Enter runs the
   whole review again (listing, refusals, dry run, open files) and
   refuses if the command, the dry run's targets or any reviewed fact
   differs.
9. **The confirm does not fit the terminal**: Enter runs nothing until
   the whole command block is visible.

A manager version other than the one the fixtures were read from is a
**warning** on the confirm, not a refusal (maintainer decision 7).

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** A removal spawn takes a `fs_gate::spawn::ToolBin` (an absolute
canonical path plus the child environment built by `tool_child_env`: fixed
`PATH`, `HOME`, `NO_COLOR=1`, pagers off, and only `DEVELOPER_DIR`,
`MISE_GLOBAL_CONFIG_FILE`, `RUSTUP_HOME` passed through), never a name.
Read-only tool invocations are an allow-list of shapes (`run_tool_read`);
removals are a second allow-list reachable only from
`fs_gate::destroy::tool_remove`. A dry-run shape without its flag is a
removal shape, which the read path refuses. Operands are typed: a mise
`<tool>@<version>` or an uppercase runtime UUID, never `all` or a flag.
`Program::Mise` has no shape in `spawn::run`, so the `PATH`-name path
refuses it. In any test build, a tool spawn whose binary is not inside
the test sandbox panics before anything starts, so no test can reach a
real manager; `SWAMP_TEST_TOOL_SANDBOX` exists only in test builds.

**Gate audit.** `gate_paths_only_inside_gates`: `tool_removal::execute`
may be named only by `crates/tui` `actions` (no CLI, JSON or agent path);
`destroy::tool_remove`, `spawn::run_tool_read` and `Program::Mise` only
by `tool_removal`.

**Runtime tests.** Listed in the front matter; each names the tempting
wrong patch it fails.
