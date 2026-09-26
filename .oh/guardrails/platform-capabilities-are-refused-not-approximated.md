---
id: platform-capabilities-are-refused-not-approximated
severity: hard
statement: "A capability the running platform does not have is refused with a named reason and leaves no state behind; it is never approximated by doing the other platform's thing badly."
outcome: disk-growth-by-project
audit: none
audit_none_reason: "2026-09-23 (Linux-on-gates port): the write-ordering half this audit checked (every install/write path asks its scheduling/continuity capability first, honours the answer, never a second time) is now a location property: every non-test write is inside `fs_gate`, and asking-then-writing outside it does not compile-check as I/O at all, so `gate_paths_only_inside_gates` (plus clippy's disallowed_methods/disallowed_types) already rejects a write that skips, discards, aliases, moves-to-a-helper or renames its way around the check -- eight of this guardrail's ten mutation fixtures are that shape and are kept, retargeted to the gated call sites, `by:`ing the existing rule. The other two are about a *second decider* of which `RefreshRefusal` a platform without replay gives; one (an unreferenced duplicate) is caught by the existing `no_unreferenced_public_items`, the other is retired (see its fixture) because a whole-file replacement's fallout lands outside the lines the mutation wrote. The retired `platform_audits::platform_capabilities_gate_their_backends` derived-call-graph rule this section documents is not replaced function-for-function; see Detection and Limits for what is and is not still checked."
runtime_tests:
  - crates/core/src/schedule.rs::tests::install_without_a_user_manager_refuses_and_writes_nothing
  - crates/core/src/systemd_user.rs::tests::no_user_manager_refuses_and_writes_nothing
  - crates/core/src/systemd_user.rs::tests::a_unit_swamp_did_not_write_is_never_replaced_or_removed
  - crates/core/src/platform/mod.rs::tests::each_platform_has_its_own_scheduler_and_neither_the_others
  - crates/core/src/fs_events.rs::tests::a_kernel_without_persisted_history_says_so_rather_than_unsupported
---

## Rationale

Every way this went wrong on Linux before the contract existed had the same shape: the code did the macOS thing, on a platform where the macOS thing means nothing, and reported success.

`swamp schedule --every 1h` wrote a LaunchAgent plist into a `~/Library/LaunchAgents` that no daemon on the machine reads, called a `launchctl` that does not exist, and printed "Scheduled observation every 1h". The user then believes a baseline is being recorded. It is not, and they find out the first time they ask a growth question — which is the moment the tool was supposed to be useful.

Free space came from `df -k`'s fourth whitespace-separated field. That is the "Available" column on macOS. GNU coreutils prints a different header and wraps a long device name onto a second line, so the same read can return a capacity percentage or nothing.

The most consequential one is continuity. `RefreshRefusal::UnsupportedPlatform` says a backend is missing — true on a platform nobody has written one for, and an invitation to write it. On Linux it is the wrong statement: the kernel keeps no change history to replay, and no implementation work changes that. #81's live watcher narrows the gap to the time before a watch opened; it does not close it. Reporting "unsupported" would promise a Linux user an incremental refresh that can never arrive. Worse, an inotify watch descriptor stored where an FSEvents event id belongs would make "swamp was not watching" read as "nothing changed" — a false measurement in a store whose whole value is that it contains none.

A refused capability is visible. An approximated one is not.

## Detection

Mechanism: runtime test.

Retired 2026-09-23 (Linux-on-gates port): the derived-call-graph `platform_audits::platform_capabilities_gate_their_backends` this section describes historically. Kept below as the record of what it checked and why each check either transferred to `gate_paths_only_inside_gates`/`no_unreferenced_public_items` (mechanism: type, gate audit) or did not survive the port (mechanism for those two: none; see `audit_none_reason` above and the retired fixture's own note).

- **Capability queries are derived**: every definition anywhere in `crates/{core,cli,tui}/src` whose return type is a capability enum (`Scheduling`, `ContinuitySource`, `Support`, `Capability`). A renamed or new query joins the set on its own.
- **Asking is not refusing** (`GUARDRAILS_SPEC.md` §17 item 2): every call that may reach a query must flow into control flow; `let _ = scheduling();` fails.
- **The guard comes before the write, transitively.** The scheduling feature is derived from the CLI's own dispatch: the definitions the `Command::Schedule` arm reaches, minus those any other subcommand reaches. Every path from there to a mutation passes through a definition that asks — exactly, not through an ambiguous namesake — honours the answer, and asks *before* it writes. Helpers called before the asking statement are walked too. Mutations are an **inverted** set: every `std::fs::` call except a short list of reads, plus `File::create`, `OpenOptions::new` and `Command::new`, so an unlisted write fails closed.
- **The platform refusal is decided once**: the `RefreshRefusal` variants built by the one function that reads `ContinuitySource` (derived, not listed) may not be constructed by any other non-test function; an exhaustive `match` that only reads a refusal is exempt by shape, not by name.

Ten rejection fixtures in the mutation corpus under `crates/source-audit/tests/mutations/platform_capabilities_gate_their_backends/`: no check, check after the write, alias/rename (`use std::fs::write as emit`), discarded answer, second refusal decider, decider that stops reading the contract, writes moved into a helper, a write through `OpenOptions`, an entry point renamed in the CLI, and an unguarded namesake of the guarded `install`.

That last one is the finding worth keeping. The first version of the audit keyed its call graph by function name, reasoning that collapsing namesakes over-approximates and so fails closed. It fails *open* wherever the graph is subtracted: `work_counters::install` is called by every subcommand, so the name `install` counted as shared and `schedule::install` — the one function the rule exists for — was never walked. Five of the corpus's installers passed that way until edges resolved to definitions.

Target gating of `Os::current` and the `CAPABILITIES` table are not text checks: `scripts/platform-isolation.sh` proves the gating on each CI target's built binary (dependency graph, linkage and symbol table), and `platform_matrix_matches_docs` holds `CAPABILITIES` and `docs/platform.md` to each other in both directions.

Runtime halves, because an audit does not prove a refusal refuses:

- `schedule::tests::install_without_a_user_manager_refuses_and_writes_nothing` (Linux) — no unit, no unit directory, no log directory, never a LaunchAgent.
- `schedule::tests::install_and_off_through_the_real_entry_points_use_systemd_units_only` (Linux) and `schedule::tests::macos_refuses_a_collector`.
- `systemd_user::tests::*` — the whole systemd lifecycle against an injected backend, on both platforms: refusal with no manager, owned units only, duplicate install/update, a failed start rolled back, uninstall.
- `platform::tests::each_platform_has_its_own_scheduler_and_neither_the_others`.

Since #83 Linux has a scheduler, so "refuse and write nothing" is now the answer to a *missing user manager* (a runtime fact) rather than to the platform; the static `Scheduling::Unavailable` refusal remains for a target with no backend at all.
- `fs_events::tests::a_kernel_without_persisted_history_says_so_rather_than_unsupported`.
- `platform::tests::a_live_watch_epoch_does_not_cover_time_before_the_watch_opened`.

## Limits

- **Dynamic dispatch.** The model resolves calls, not trait-object dispatch or function pointers. A future scheduling backend reached through a `dyn` trait is invisible to the walk; the runtime tests above (which assert the plist, agents and log directories are *empty*, not that a particular call was absent) are what catch it.
- **Shared helpers.** Code reachable from any other subcommand is subtracted from the scheduling feature, so a writing helper that `observe` also calls, invoked by `install` before its guard, is not walked. The subtraction uses exact call edges only, so ambiguity never widens it; the residual is a helper that genuinely is shared.
- **Methods.** A method call resolves to every definition with that name (methods are not aliased, and the receiver type is unknown to a syntax-level model). That over-approximates the walk, which fails closed — but an ambiguous method call cannot count as the *guard*: only a call that reaches exactly one definition, and that definition a query, does.
- **Statement granularity.** "Before" is by top-level statement. A write inside the same statement as the question (`if can() { write() }`) is treated as behind it, which is right for the shape guards are written in and wrong for a write placed in the refusal branch itself.
