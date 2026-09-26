---
id: disk-growth-by-project
kind: outcome
status: active
s_and_t_step: W0
parent_step: null
sufficiency_group: G0
owner: null
review_trigger: "A new detector, scope change, contradictory fact, or real user decision exposes a failure."
tactic_disposition: selected
mechanism: |-
  Developers running many coding agents keep free space workable without emergency scans.
  The tool answers "what grew on this disk, by project, and what do I do about it" from a
  persistent column store of observations, and lets a human or an agent act on the answer
  with the facts in front of them. The agent is the everyday operator; only a human
  authorizes anything destructive.
files:
- crates/core/src/walk.rs
- crates/core/src/growth.rs
- crates/core/src/fs_events.rs
- crates/core/src/bus/*
- crates/core/src/consumers/*
- crates/core/src/report.rs
- crates/core/src/agent_json.rs
- crates/cli/src/main.rs
- skills/swamp/*
- crates/tui/src/*
- .oh/guardrails/*
---

# Disk growth by project, acted on with facts

Technical constraints are recorded under `.oh/guardrails/`. Existing source audits
are listed by `cargo run -p swamp-source-audit -- --list`. Recording a constraint
does not establish that it is implemented: the new W1b and W2a records below name
validation gaps and do not claim executable enforcement.

## Desired behavior change — W0

Developers understand what grew and make justified keep / remove / investigate
decisions without opening every project or reconstructing its context by hand.
This elaborates the existing outcome, including the agent-operated workflow;
it does not replace human authorization for destructive actions.

## Mechanism — selected direction, outcome still a hypothesis

Understandable scan coverage, trustworthy observation history, and relevant
project, activity, and recovery evidence together reduce outside investigation.
More indexed paths or more confident-looking recommendations are not success.
Built-in detectors and configuration differences are now selected in the
[full-scope solution-space session](../sessions/2026-09-19-developer-storage-coverage-and-evidence.md).
Concrete implementation choices must preserve that session's contracts and checks.

## Feedback

For representative real keep / remove / investigate decisions, record which
outside checks remain necessary and whether the supplied facts change the choice.
Review after those decision episodes and when new coverage or evidence is added.
No numeric target or benchmark has been established. Fewer checks achieved through
misleading reassurance invalidate the mechanism rather than demonstrate success.

## Strategy and tactics lineage

Source: the 2026-09-19 conversation's "Problem weave: overview"; recorded at the
user's request for W0, W1, W2, W1b, and W2a. Recording initially left tactics as
candidates. The subsequent solution-space pass selects W0, W1, W1a, W1b, W2, W2a,
W2b and W3 for the full scope at the user's request. Ownership remains unassigned.

- **G0**, parent W0, children W1 + W2 + W3, **all required**: understandable
  measurements, consequence evidence, and sustainable routine usefulness jointly
  support informed decisions. Actual decision improvement still needs testing.
- W1: [coverage-aware storage history](coverage-aware-storage-history.md).
- W2: [decision-relevant storage evidence](decision-relevant-storage-evidence.md).
- W3: routine usefulness is selected in the session, without a separate outcome-family
  record. Its absence from the record set does not remove it from G0.

## Assumptions and boundaries

Evidence must be obtainable at tolerable cost and must answer decision-relevant
questions. Scanning is not authorization. Unknown is not unused or safe;
unobserved is not deleted. Measurement can be useful before ownership is known.
Shared artifacts need not have a single project owner. No whole-disk completeness,
universal last-use detection, or guaranteed recoverability is implied.

Reassess if broader coverage mostly adds noise, outside investigation is unchanged,
or missing evidence is interpreted as permission to delete. No migration support or
implementation is introduced by these records; issue planning follows the linked
solution-space selection.

## Guardrails
- fsevents-before-full-walk
- column-store-parquet-zstd
- reverse-delta-current-plus-deltas
- scheduled-refresh-launchagent
- folding-only-for-artifacts
- symlinks-never-followed
- incremental-walk-only-changed-subtrees
- walk-optimized-parallel-pool
- dir-mtime-int32-minutes
- agent-interface-facts-not-verdicts
- human-only-authorization
- event-bus-pluggable-consumers
- extractors-are-pluggable
- one-byte-formatter
- computed-but-not-delivered
- coverage-changes-are-not-storage-changes (W1b; validation pending)
- activity-and-consumer-evidence-have-limits (W2a; validation pending)
