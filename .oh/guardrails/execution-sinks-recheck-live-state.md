---
id: execution-sinks-recheck-live-state
severity: retired
statement: "Retired 2026-09-23 by product decision: swamp reports, the human removes. There is no recheck-then-veto gate in front of a Trash move any more: `crate::recheck` and `crate::authority` are deleted, and `fs_gate::destroy::trash_move`/`Envelope` take a plain path. The TUI shows current facts (path, size now, what it is, what deleting it costs, who has it open) at Backspace, and Enter moves exactly what was marked -- with NO re-derivation, NO 'changed since review' refusal, and NO occupancy veto in between. The only refusals left are ordinary OS errors (permission denied, the path is gone, cross-device)."
outcome: decision-relevant-storage-evidence
---

## Why this was retired

This guardrail described `crate::recheck`: a live re-derivation
(reviewed identity/membership, protection loaded fresh, tri-state
occupancy) that every destructive sink had to run immediately before its
first destructive call, refusing on any drift since the plan/confirm
step. That model, and the `RecheckProof`/`Authorized` types that carried
it, are deleted as of stack/27 (2026-09-23, "swamp reports; the human
removes") along with the plan/grant/CLI-execute path it protected.

The product decision removes the thing this guardrail was protecting
(an automated action pipeline), not just its enforcement mechanism.
What remains: the TUI still shows a human the facts before they press
Enter (`crates/tui/src/app.rs`'s confirm banner), but there is no
second automated check standing between "the human decided" and "the
Trash move happens" -- exactly as there would not be if the human ran
`mv` themselves after reading `swamp report`.

`trash-backend-owns-every-move.md` still applies: every Trash move goes
through `fs_gate::destroy`, one function, never a `std::fs::rename`
elsewhere -- that discipline is about *where* a move happens, not about
gating it on a recheck.

## Detection

Mechanism: type. There is no runtime check left, and none is needed: the
mechanism this guardrail described is deleted, not replaced. What still
holds -- every Trash move going through `fs_gate::destroy` -- is
`trash-backend-owns-every-move.md`'s own gate audit, not this one's.
