---
id: tool-removal-refuses-on-manager-facts
severity: hard
statement: "Specific to removal with no undo (tool-managed removal, #177: `mise uninstall <tool>@<version>`, `xcrun simctl runtime delete <UUID>`; no set form). There is no Trash to fall back on, so this one action class keeps automated refusals and a review-to-Enter recheck that every Trash move gave up on 2026-09-23. A removal runs only after the manager's own dry run, only through a manager binary resolved from a fixed list of directories with an environment built from nothing, only from the human's `Y` on the TUI confirm (never Enter, never before the confirm has been drawn in full and 1 s has passed), and only if a full re-review at Enter finds nothing changed. Every refusal below is fail-closed: a fact that could not be read refuses."
outcome: decision-relevant-storage-evidence
audit: gate_paths_only_inside_gates
runtime_tests:
  - crates/core/tests/tool_removal_adversarial.rs::global_config_version_refused_even_though_mise_dry_run_exits_0
  - crates/core/tests/tool_removal_adversarial.rs::source_null_from_cwd_is_not_no_consumer
  - crates/core/tests/tool_removal_adversarial.rs::no_prune_removal_shape_exists
  - crates/core/tests/tool_removal_adversarial.rs::a_changed_dry_run_text_or_reason_refuses_at_enter
  - crates/core/tests/tool_removal_adversarial.rs::backend_tool_names_mise_prints_are_operands
  - crates/core/tests/adv_g5.rs::adv_dry_run_path_with_dotdot_escaping_mise_dirs_refuses
  - crates/core/tests/adv_g5.rs::adv_ls_duplicate_entries_are_not_resolved_by_first_match
  - crates/core/tests/adv_g5.rs::adv_device_booting_or_without_state_refuses
  - crates/core/tests/adv_g5.rs::adv_untrusted_developer_dir_never_reaches_xcrun
  - crates/core/tests/adv_g5.rs::adv_install_dir_that_is_a_symlink_out_refuses
  - crates/core/tests/adv_g5.rs::adv_append_to_an_unreadable_ledger_never_drops_history
  - crates/core/tests/adv_g5.rs::adv_a_sandbox_rooted_at_a_system_dir_does_not_resolve_a_real_manager
  - crates/tui/tests/tool_removal_sheet.rs::no_enter_ever_runs_a_removal
  - crates/tui/tests/tool_removal_sheet.rs::y_runs_only_after_the_confirm_has_been_seen_for_the_hold_off
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

1. **Manager not found, or not trusted.** `fs_gate::program_paths` (the
   one resolver, shared with the manager probe) looks only in a fixed
   list (`/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`,
   `~/.cargo/bin` for mise; `/usr/bin/xcrun`), never on `PATH`. The first
   candidate that exists must be (canonically) a file owned by the user
   or root, writable by nobody else, in a trusted directory, or it
   refuses: it is never skipped for a later one. A trusted directory is
   owned by the user or root, not world-writable, and group-writable only
   for the macOS `admin` group (looked up by name): admin members can
   already use sudo, so that grants no new power, and it is how standard
   Homebrew keeps `/opt/homebrew/bin`. Any other group, another owner, or
   group-write on Linux refuses, naming the group. The child's `PATH`
   starts with the candidate's directory only when that is trusted too.
2. **A config requests the version** (`source` in `mise ls`): the global
   config (mise's config dir from its own env, or
   `MISE_GLOBAL_CONFIG_FILE`) is named as such.
3. **mise's list cannot be read, or is ambiguous**: a `source` without a
   readable path, a non-string or non-bool field where one is expected;
   the version listed twice with any entry requested, active or
   unreadable.
4. **mise reports the version active, it is a symlink install, it is not
   exactly `<data dir>/installs/<tool folder>/<version>`, or a directory
   from `installs` down to it is a symlink** (lstat of every component).
5. **mise's prune does not list the version.** Only prune knows the
   configs mise tracks, so a per-version removal is offered only inside
   prune's own set. Versions pinned by `MISE_<TOOL>_VERSION` in the
   user's shell are invisible to swamp, and the confirm says so.
6. **The dry run did not run cleanly**: a non-zero exit, a timeout,
   output over 1 MiB, an `ERROR` line, a line swamp does not recognize, a
   missing dry-run marker (`✓ uninstalled (dry-run)`, simctl's single
   `Would delete ... <UUID>` line), a configuration-links line, a path
   that is not absolute, holds `..`, or is not exactly this version's
   install or cache directory (component by component), or not exactly
   the target.
7. **A simulator on the runtime is anything but exactly `Shutdown`**, in
   the default set or the Xcode Previews set (when simctl cannot show the
   Previews set, the confirm says other device sets are not visible). A
   runtime without an identifier, without a mount path (the open-file
   check cannot run), not `deletable`, or not `Ready`/`Unusable` refuses.
   Unbooted devices do not refuse: the confirm names how many and which
   (maintainer decision 5).
8. **Files are held open**, or the open-file check could not finish
   (`Unknown` refuses: maintainer decision 4). The one holder that does
   not count is CoreSimulator's own host process, by exact command name
   (`SimLaunchHost.arm64`, `SimLaunchHost.x86_64`, `SimLaunchHost`) and
   only when its own executable is under CoreSimulator's or Xcode's
   folder; simctl stops it before deleting.
9. **Anything changed between the confirm and `Y`**: `Y` runs the whole
   review again (listing, refusals, dry run, open files) and refuses if
   the command, the dry run's full text, the manager's own reason lines,
   its targets or any reviewed fact differs. What changes in the
   milliseconds between that re-review and the command starting is not
   seen: the re-review narrows the window, it does not close it.
10. **The confirm was not seen**: Enter never runs a removal; `Y` runs
    nothing until the confirm has been drawn in full at a known terminal
    size and 1 s has passed; input queued when it appears is dropped;
    bracketed paste is on and a paste is never keys.
11. **The ledger cannot be written**: a `started` row is written before
    the command runs (replaced by the outcome), and if the ledger cannot
    be written at all nothing runs. An unreadable ledger is kept aside as
    `ledger.parquet.corrupt-<time>`, never overwritten.

A manager version other than the one the fixtures were read from is a
**warning** on the confirm, not a refusal (maintainer decision 7). The
child environment is `HOME`, a fixed `PATH`, `NO_COLOR=1`, `LC_ALL=C`,
pagers off, and for mise exactly `MISE_DATA_DIR`, `MISE_CONFIG_DIR`,
`MISE_CACHE_DIR`, `MISE_STATE_DIR`, `MISE_GLOBAL_CONFIG_FILE`,
`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`;
for xcrun a `DEVELOPER_DIR` only when validated (a real directory owned
by the user or root, not writable by others, holding a trusted `simctl`),
otherwise "DEVELOPER_DIR ignored" on the confirm and the developer dir
used in the ledger row.

Not locked: two swamp processes against each other. A second instance
confirming the same removal finds the manager with nothing to do (a
no-op or an error), and the ledger's read-then-rewrite can lose one of
the two rows (a store lock is a filed follow-up). Within one process,
tool removals run one at a time.

Not built: a bulk `mise prune --tools` removal (a set decided at exec
time, whose path list cannot always be shown), `brew`, `rustup`.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** A removal spawn takes a `fs_gate::spawn::ToolBin` (an absolute
canonical path plus the child environment built by `program_paths::child_env`), never a name.
Read-only tool invocations are an allow-list of shapes (`run_tool_read`);
removals are a second allow-list reachable only from
`fs_gate::destroy::tool_remove`. A dry-run shape without its flag is a
removal shape, which the read path refuses. Operands are typed: a mise
`<tool>@<version>` or an uppercase runtime UUID, never `all` or a flag.
`Program::Mise` has no shape in `spawn::run`, so the `PATH`-name path
refuses it. In any test build, a tool spawn whose binary is not inside
the test sandbox panics before anything starts, so no test can reach a
real manager; `SWAMP_TEST_TOOL_SANDBOX` exists only in test builds, and a sandbox outside the system temp dir, a link, or a system directory (or one holding one) resolves nothing; a sandbox whose ledger cannot open panics rather than use the real one.

**Gate audit.** `gate_paths_only_inside_gates`: `tool_removal::execute`
may be named only by `crates/tui` `actions` (no CLI, JSON or agent path);
`destroy::tool_remove`, `spawn::run_tool_read` and `Program::Mise` only
by `tool_removal`.

**Runtime tests.** Listed in the front matter; each names the tempting
wrong patch it fails.
