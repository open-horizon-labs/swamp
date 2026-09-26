---
id: human-only-authorization
severity: retired
statement: "Retired 2026-09-23 by product decision: swamp reports, the human removes. There is no swamp-enforced authorization boundary any more to be human-only about -- no plan store, no grant, no confirmation token, no CLI approve/execute. The CLI and skill are read-only (`report`/views/JSON); the only thing that moves a path to the Trash is the TUI's own Space/Backspace/Enter flow, or a human at a shell. What used to be a code-review boundary between 'authorized' and 'not authorized' is now just: a human's keyboard, and the facts swamp showed them before they pressed Enter."
outcome: disk-growth-by-project
---

## Why this was retired

Through v0.6.x this guardrail described a real mechanism: a
`HumanConfirmed` token minted at one reviewed call site, spent by a
`Plan`/`Grant` the store loader verified against a keyed binding, and an
`Authorized` value every destructive sink required before it would touch
anything. That mechanism -- `crates/core/src/authority.rs`,
`crates/core/src/recheck.rs`, the `Plan`/`Grant` persistence in
`actions.rs`, `fs_gate::key`'s authority key, and the CLI's
`propose`/`propose-agents`/`approve`/`execute`/`grant`/`plans`/
`cleanup-check` commands -- is deleted as of stack/27 (2026-09-23,
"swamp reports; the human removes").

The product decision is: swamp's job is to report facts, not to decide
or execute anything. The TUI keeps a Trash flow (Space marks, Backspace
shows current facts, Enter moves the marked paths to the platform
Trash and appends one ledger line), but there is no plan/grant/token in
front of that move -- the only refusals left are ordinary OS errors
(permission denied, the path is gone, cross-device). See
`skills/swamp/references/trust-model.md` for the current statement of
what swamp is (a read-only tool) and who decides (a human with a
keyboard).

`tui-actions-off-event-thread.md` is unaffected: the TUI's Trash move
still must never run on the render/event thread, for the same reason it
never did.

## Detection

Mechanism: type. There is no runtime check left, and none is needed: the
authorization mechanism this guardrail described is deleted, not
replaced by another check. What actually still limits harm -- a human
must be at the keyboard to run `swamp` or open its TUI at all -- was
never something this guardrail (or any audit) could detect; it is an
operating-system fact, not a swamp one.
