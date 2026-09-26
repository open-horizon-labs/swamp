# Linux x86_64 support with native platform backends

## Aim

Use swamp on Linux x86_64 boxes as well as macOS: understand project storage and growth, inspect through CLI/TUI/MCP, collect history and perform supported reviewed actions without losing correctness or platform-specific efficiency. This is a separate outcome from #40; neither #40 nor #74 is a prerequisite.

## Problem Space

The user requested Linux x86_64 shims and target-specific optimization/builds. Asked about distribution/systemd, they said “I dunno you tell me. Ubuntu probably?” Planning baseline: Ubuntu 24.04 x86_64 with systemd, GNU/glibc target; this is a support choice, not verified fleet inventory. Actual host validation remains required. Release validation includes the baseline and a newer supported Ubuntu LTS chosen at implementation time. Windows, Linux ARM, musl/static universal binaries and all-distro support are not implied.

Inspection: core fs_events.rs already target-gates macOS bindings and otherwise refuses incremental replay; Cargo core dependencies for FSEvents/CoreFoundation are target-gated. walk.rs is a bounded parallel std::fs/Unix metadata walker, not a macOS-only bulk-syscall implementation. growth.rs persists FSEvents-shaped state and device-number keyed history. schedule.rs invokes launchctl and uses Library paths. Core and TUI each contain Trash/free-space helpers; occupancy.rs relies on lsof. Source audits directly require FSEvents replay and launchctl. release.yml builds only macOS arm64; no other CI workflow was present.

Inotify does not provide persistent historical replay while no instance is watching. Recursive watch installation, queue overflow, watch limits, rename races and service gaps require explicit coverage/continuity handling. A timer alone does not keep an inotify instance alive. Native optimizations must preserve shared semantics, not fork report/history logic.

## Selected approach

Shared semantic core with compile-time selected platform adapters and Cargo target-specific dependencies. Keep portable traversal shared; benchmark Linux before adopting a specialized fast path. Preserve macOS FSEvents replay/launchd; add Linux inotify with explicit epochs, optional user-owned continuous collection, correct gap reconciliation, systemd user lifecycle, Linux paths, recoverable Trash and conservative occupancy.

Plain CLI/TUI/MCP works without a service; no background installation or privileged changes by default. Native releases contain only applicable OS adapters plus shared core. Generic x86_64 release CPU baseline; no target-cpu=native. Verify glibc/kernel/linkage requirements from actual artifacts.

Rejected: compiling both OS backends and choosing at runtime; pretending Linux has a persisted FSEvents cursor; full-scan-only as completed Linux optimization; a mandatory privileged daemon; duplicating the walker without measurement. Full reconciliation after unknown coverage is required correctness, not a failure to optimize.

## S&T selection

- L0 selected: independent Linux usability outcome, parent none.
- L1 selected: correct native observation and shared core contracts, parent L0. Necessity: unsupported replay cannot provide efficient trustworthy updates. Assumption: native watcher/traversal and explicit continuity can preserve semantics.
- L2 selected: native lifecycle and safe user-facing operation, parent L0. Necessity: a buildable scanner without correct scheduling/paths/actions is not usable parity. Assumption: unprivileged OS adapters can preserve existing protections.
- L3 selected: native validation and separate releases, parent L0. Necessity: cross-compilation alone does not prove runtime or performance support. Assumption: native tests, artifacts and measured workloads reveal failures.

GL = L1 + L2 + L3, all required and selected. Owners unassigned. Review trigger for all: unsupported capability/continuity, false history, unsafe action, incompatible binary, macOS regression or unacceptable Linux cost. No concrete public schema is mandated; review one-way architecture choices before implementing them.

## Risk retirement and handoff

Planned tests, not claimed passed: queue overflow/new subtree/bootstrap mutations defeat empty-change-list false confidence; stopped-process/reboot/mount replacement defeats persisted-inotify-cursor fiction; sparse/hardlink/permission fixtures defeat incorrect accounting; Trash metadata/cross-volume/occupancy tests defeat irreversible fallback or missing-visibility-as-safe; target dependency/linkage/native-run checks defeat hidden cross-OS dependencies and build-host CPU assumptions. Compare optimized traversal with reference semantics and benchmark initial/steady-state/gap recovery separately.

Accepted limitations: future-use and inaccessible process/filesystem facts can remain unknown; non-systemd hosts retain manual use and explicit scheduler unavailability; watch gaps require reconciliation. Actual user machines are unverified. Human acceptance: install/use on the eventual Linux host and review timing/usability. No real cleanup, service install, implementation or release happens during planning.

Primary references: https://man7.org/linux/man-pages/man7/inotify.7.html ; https://specifications.freedesktop.org/trash/latest/ ; https://releases.ubuntu.com/24.04/ . Verify systemd user/timer and XDG documentation during implementation; direct systemd documentation fetch was unavailable during planning.

## Plan

**Outcome:** [#77](https://github.com/open-horizon-labs/swamp/issues/77), separate from #40.
**Delivery epic:** [#78](https://github.com/open-horizon-labs/swamp/issues/78).
**Status:** Planning only; 11 native sub-issues created and dependencies recorded. No application code, dependencies, builds, services or releases changed.

| S&T Step | Disposition | Issue/Epic | Parent Step | Depends On |
|---|---|---|---|---|
| L0 | selected | [#77](https://github.com/open-horizon-labs/swamp/issues/77) | none | independent outcome |
| L1/L2/L3 | selected | [#78](https://github.com/open-horizon-labs/swamp/issues/78) | L0 | children below |
| L1 | selected | [#79: Isolate platform backends with target-gated dependencies and shared capability contracts](https://github.com/open-horizon-labs/swamp/issues/79) | L0 | none |
| L1 | selected | [#80: Validate and optimize Linux traversal, allocation accounting and volume identity](https://github.com/open-horizon-labs/swamp/issues/80) | L0 | #79 |
| L1 | selected | [#81: Implement unprivileged Linux change watching with complete-coverage and overflow handling](https://github.com/open-horizon-labs/swamp/issues/81) | L0 | #79, #80 |
| L1 | selected | [#82: Track Linux watcher continuity and reconcile observation gaps honestly](https://github.com/open-horizon-labs/swamp/issues/82) | L0 | #81 |
| L2 | selected | [#83: Add opt-in Linux user scheduling and watcher lifecycle without changing macOS launchd behavior](https://github.com/open-horizon-labs/swamp/issues/83) | L0 | #82 |
| L2 | selected | [#84: Use Linux-native data locations and platform-specific discovery capabilities across interfaces](https://github.com/open-horizon-labs/swamp/issues/84) | L0 | #79 |
| L2 | selected | [#85: Reuse a vetted cross-platform Trash backend behind shared safe action handling](https://github.com/open-horizon-labs/swamp/issues/85) | L0 | #79, #84, #80 |
| L2 | selected | [#86: Make Linux occupancy and action rechecks explicit and fail closed on missing visibility](https://github.com/open-horizon-labs/swamp/issues/86) | L0 | #79, #80 |
| L3 | selected | [#87: Run native Linux x86_64 and macOS CI with platform-isolation and behavior tests](https://github.com/open-horizon-labs/swamp/issues/87) | L0 | #81, #82, #83, #84, #85, #86 |
| L3 | selected | [#88: Ship separately packaged Linux x86_64 and macOS artifacts with verified compatibility](https://github.com/open-horizon-labs/swamp/issues/88) | L0 | #87 |
| L3 | selected | [#89: Validate Linux usability and platform-specific optimization without macOS regressions](https://github.com/open-horizon-labs/swamp/issues/89) | L0 | #88 |

Full scope is the combined L1/L2/L3 set; do not close on a compiling binary or manual full-scan-only port. Split oversized tasks retaining acceptance criteria rather than dropping capabilities. Start #79 with reuse assessment before architecture commitment; this planning request does not start implementation.

### User follow-up: reuse generic tools; planning only

The user pointed to https://github.com/clean-dev-dirs/clean-dev-dirs and explicitly reiterated no implementation. Reviewed its Cargo.toml and public lib.rs: it exports Scanner/project/detection functionality and depends on trash, walkdir and jwalk. Prefer evaluating those APIs/libraries rather than writing equivalent OS plumbing. Also assess notify for target-native watching and explicit polling fallback. Existing swamp ecosystem rules already cite clean-dev-dirs; avoid conflicting duplicate catalogs.

#79 requires a reuse decision including licenses, API/dependency/build-script footprint, error visibility, target gating and maintenance. #80 evaluates generic traversal/reference/fallback; #81 evaluates notify without replacing retained FSEvents replay; #84 considers read-only upstream detectors; #85 prefers vetted Trash reuse with narrowly justified adapters. Generic cross-platform source is compatible with target-only builds when irrelevant OS implementations/dependencies are cfg-excluded. Custom code requires a demonstrated correctness or measured performance gap.

Review trash's documented Linux mount-query thread-safety caveat and recovery-location/ledger support before adoption. Do not call a cleaner's default destructive CLI as a scan. No dependency is selected merely from documentation; exact versions and semantic fit are implementation-time evidence gates. Primary references: https://docs.rs/trash/latest/trash/ ; https://docs.rs/notify/latest/notify/ ; https://github.com/clean-dev-dirs/clean-dev-dirs/blob/main/Cargo.toml .

## Reconciliation, 2026-09-21

#103/#104 landed: `crates/mcp` is removed. Where this session says
"CLI/TUI/MCP", read that as "CLI (interactive and `--json`) and TUI" --
Linux native support inspects and acts through those two, plus the
`skills/swamp/` agent skill, with no separate MCP server to package or
target-gate. "Plain CLI/TUI/MCP works without a service" still holds
for "plain CLI/TUI": no background installation or privileged changes
by default.
