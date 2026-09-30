---
id: occupancy-is-tristate-at-sinks
severity: retired
statement: "Retired 2026-09-23 by product decision: swamp reports, the human removes. There is no sink-side occupancy veto any more -- `crate::recheck` (which consumed OccupancyState to refuse a move) is deleted along with the rest of the CLI action path. Occupancy is still computed tri-state (`occupancy::OccupancyState::{Free, Occupied(path), Unknown(reason)}`) and still shown to the human on the TUI's confirm banner as a fact, but nothing gates a Trash move on it: `Unknown` is displayed, never a refusal."
outcome: decision-relevant-storage-evidence
---


**Exception (2026-09-30, #177):** tool-managed removal (a mise version, a simulator runtime), which has no Trash, keeps a review-to-confirm recheck and a TUI-only, human-only confirm: `.oh/guardrails/tool-removal-refuses-on-manager-facts.md`. **Amended the same day:** an occupied or unanswered open-file reading is a *warning* there too (said as held, or as not checked), not a refusal; `Y` refuses only if the reading changed since the confirm. A Trash move (Reclaim, External, Disk, artifacts, agents) names the same tri-state reading on its confirm and is never gated on it.

## Why this was retired

`open_cache_member_must_stop_parent_removal` (the 2026-09-21 review's
counterexample this guardrail was written for) asserted that a sink
must refuse to move a directory while a member inside it is open. That
assertion is no longer true by design: the product decision removes
every automated veto in front of a Trash move, including this one. A
human who presses Enter with a file open somewhere inside what they
marked gets the move -- the open-file fact was shown on the confirm
banner first, and it is their call.

`OccupancyState` itself (`Free`/`Occupied`/`Unknown`, no boolean
conversion) is unchanged and still used by `occupancy::probe_path`/
`probe_paths` to build the evidence shown on that banner --
`.oh/guardrails/occupancy-gaps-are-unknown-never-free.md` (if present)
or the general facts-not-verdicts discipline still applies to how it is
*worded*. What is retired is only the refusal that used to sit between
the reading and the move.

## Detection

Mechanism: type. There is no runtime check left, and none is needed:
the veto this guardrail described is deleted, not replaced. What
remains true -- `OccupancyState` has no boolean view, so nothing can
collapse `Unknown` into "free" by accident when it *is* consulted for
display -- is enforced the same way it always was (the type itself),
just no longer at a destructive sink, because there is no destructive
sink left that consults it.
