---
id: scheduled-refresh-launchagent
severity: hard
statement: "Where the platform has a user scheduler, a scheduled observation exists as an opt-in per-user job so a previous observation is there without a human running anything. Where it does not, the command refuses with a named reason and writes nothing -- it never installs a job that cannot run."
outcome: disk-growth-by-project
audit: gate_paths_only_inside_gates
compile_fail:
  - launch_agent_is_a_named_text_file
runtime_tests:
  - crates/core/src/schedule.rs::tests::plist_golden_content
  - crates/core/src/schedule.rs::tests::off_removes_plist_and_issues_bootout_in_test_mode
---

## Rationale
Growth needs a baseline. Without a schedule the first question a user asks has no history behind it.

## Detection

Mechanism: type, gate audit, runtime test.

**Type.** The plist is written only as `TextFile::LaunchAgent` (validated to `<label>.plist`); the systemd unit is written only through the same store gate.

**Gate audit.** `Program::Launchctl` may be named only in `schedule`; `Program::Systemctl` and `Program::Loginctl` may be named only in `fs_gate::spawn`'s own shape table, reached only from `schedule`'s Linux backend.

Retired 2026-09-22: the `scheduled_refresh_launchagent` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `launch_agent_is_a_named_text_file`.

The platform-neutral half -- every `install` consults the scheduling capability, honours the answer, and does so before any write -- is audit `platform_capabilities_gate_their_backends`, with the runtime half in `schedule::tests::install_without_a_user_manager_refuses_and_writes_nothing` (Linux: the `systemd --user` backend refuses and writes nothing where no user manager is reachable) and the injected-backend lifecycle tests in `systemd_user::tests`.

## Limits
On Linux the backend talks to `systemd --user` through `fs_gate::spawn`; where no user manager is reachable (no session bus, no root) it refuses by the same rule rather than approximating one, and writes nothing.
