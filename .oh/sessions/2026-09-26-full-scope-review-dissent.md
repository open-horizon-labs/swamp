# Full-scope review and dissent — 2026-09-26

Reviewed PR #144 at `2b7ee70f7072303ac37afa99d9c4096a1e7e4ff4` against
`.oh/handoffs/2026-09-21-claude-full-scope.md`, current source, open issue
acceptance, and the locally installed release-mode build of that commit.
This is an independent targeted review, not an exhaustive audit of all 815
changed files. No real cleanup was performed.

## Review

**Aim:** Help a developer see where agentic development consumes disk,
what grew, which project owns it, and what can be removed with understandable
consequences, without manual archaeology or audit-perfect prerequisites.

**Status: Adjust.** Useful progress, but not completion of the full packet.

### Findings

1. **P1: agent removal previews misstate recovery.**
   `agents/copilot_cli.rs:267–279` identifies command recall history, but
   classifies it as `Caches` with `CacheOrLogTrash`. `actions.rs:996–1023`
   turns that capability into "regenerated automatically" and
   `local_rebuild`. New history/log files can be created; deleted past
   records cannot thereby be recovered. The same blanket promise applies
   to diagnostic logs. This is not a reason to prohibit removal: retain
   practical Trash actions, but distinguish historical-data loss from
   rebuildable cache loss in the actual confirmation and recovery model.
   Add a fixture that checks the produced plan's consequence, not just its
   action enum. This directly violates the packet's consequence contract.

2. **P1 for full-scope completion: cross-ecosystem build cleanup is absent.**
   `tui/src/model.rs:1812–1816` explicitly leaves every non-Cargo interior
   unselectable because no executor exists. `build_adapters/matrix.rs`
   advertises inspection-only actions throughout. #73 remains open and
   explicitly requires precise actions across implemented adapter families.
   Identification is valuable and independently releasable; it does not
   satisfy the full handoff. Do not solve this with recursive deletion of
   arbitrary identified directories. Deliver tested, bounded operations
   for supported rebuildable units, starting with Node as requested.

3. **P2: timestamp evidence still contradicts the repaired presentation.**
   `activity.rs:68–72` calls any mtime after the observation start clock skew
   or future-dated. `render.rs:1667–1680` already recognizes that writes
   during a scan are ordinary and tolerates them. The structured evidence
   still carries the misleading warning. Observation timestamps are captured
   before the scan (`report.rs:1748`, `2115`). Test a write during observation
   separately from a genuinely future-dated fixture; use observation-window
   semantics consistently rather than fixing only the displayed age.

4. **P2: ordinary shell truncation produces a panic.**
   Reproduced `swamp observe | head -2`: the observation completed and
   persisted, then stdout printing panicked with Broken pipe. Handle EPIPE
   gracefully at the output boundary. This is a usability defect, not
   evidence of observation corruption.

5. **Known CLI defects assigned separately to Luna:** Rust JSON ignores
   project filtering; Rust text recommends nonexistent `cleanup-check`.
   Both undermine the CLI/skill replacement. Worker scope includes public
   behavior regression tests and a PR stacked on #144, not the findings above.

6. **Explicit remaining scope:** #107 on-demand per-crate diagnostics is
   open; Cargo's matrix explicitly says it is not attempted. Some agent
   catalog entries are narrower than the packet: Copilot session linkage
   is always unresolved and session actions are disabled. Unknown schemas
   warrant honest limits, but a catalog entry is not evidence that all
   linkage/action acceptance is delivered. Missing project identity alone
   must not become a universal removal prohibition; exact membership and
   reference/occupancy safety are separate questions.

### Alignment check

- Necessary: project attribution, folded inventory, history and practical
  actions still address the original need.
- Aligned: broad discovery, state-index Codex linkage, Parquet persistence,
  native platform paths and consequence-first UI preserve the direction.
- Sufficient: no, particularly non-Rust actions and the required diagnostic.
- Mechanism clear: identify units, retain folded measurements, invalidate
  changed containers, attach evidence, then review exact supported actions.
- Complete: no; full native CI is useful evidence, not all issue acceptance.
- Risks retired: partially. Current runtime supports incremental operation;
  recovery-language accuracy and full workflow coverage remain unresolved.

### Measurements and contrary evidence to a blanket rejection

The prior initial live observation took 48.294 seconds. This review's
observation reused the stored event anchor: 6.990 seconds, incremental,
288 changed directories, 38 projects, 58 external units, 5,946 agent units.
The immediate follow-up was 4.452 seconds and 12 changed directories.
A stored project JSON report took 0.75 seconds wall time.
These are active-machine measurements, **not** zero-change benchmarks;
they refute a claim that every observation still performs a full walk.
The second observation's output was truncated by head and exposed finding 4.

Codex reads its existing versioned SQLite state index for `rollout_path` and
`cwd`; that is an upstream metadata source, not a new Swamp scan-state DB.
No request to restore transcript scanning follows from unresolved links.
Homebrew default-off was owner-approved, not unauthorized scope reduction.

### Frame and drift

The frame remains sound; no restart is indicated. The drift is **completion
drift**: a broad implemented catalog plus strong structural checks can look
like the completed understand-and-act workflow while operations are absent.
The consequence enum also collapses distinct data losses into one cache
story. Route both to Adjust, not more speculative infrastructure.

### Risk retirement and human verification

- Two native full-check jobs passed on the reviewed head in the preceding
  status check. Mutation fixtures test selected architecture constraints;
  they do not prove all product consequences or every adapter's usefulness.
- No deletion experiment was run on real user data. Disposable fixtures
  remain the right place for recovery/retained-data tests.
- Live no-change isolation, Linux end-user installation/usability, and
  actual reclaim accounting were not independently re-measured in this pass.
- Human checkpoint: does the TUI recommendation let the owner choose without
  another explanatory conversation? A model-generated review cannot certify
  that experience. Do not use this checkpoint to defer reproducible code bugs.

## Dissent

**Decision:** Is this implementation ready to count as completion of Claude's
full packet? **Confidence before: medium; recommendation: ADJUST.**

### Steel-man

The implementation invests in shared typed boundaries, folded accounting,
explicit capability limits and platform tests so many adapters can share one
safe storage workflow. Keeping unsupported operations unavailable is better
than inventing destructive scope. Incremental measurements show real benefit.

### Contrary evidence

The product still presents non-Rust groups without operations, publishes
incorrect recovery promises for historical records, and required #107 is
explicitly absent. Two basic CLI contract failures survived green gates.
Therefore neither breadth nor gate count establishes completion.

### Pre-mortems

1. Functional: someone discards command history expecting it to regenerate;
   action eligibility was confused with recovery semantics.
2. Adoption: impressive catalog coverage still sends the owner to another
   agent for deletion instructions because common build groups cannot act.
3. Opportunity cost: more audits and catalog labels consume effort while
   precise Node cleanup and ordinary workflow regressions remain undone.

### Assumptions to test

| Assumption | Evidence | Risk | Test |
| --- | --- | --- | --- |
| Cache/log action implies regeneration | Generic plan code; contradicted by command recall history | Misleading consent | Assert exact loss/recovery text for history, logs and rebuildable caches |
| Implemented adapter means usable workflow | Identification matrix only | False completion | Identify → select → preview → fixture removal → retained-data/rebuild check per supported family |
| Full mutation gate proves release readiness | Selected structural mutation corpus | Product bugs escape | Real command contract and fresh-user workflow trials |
| Warm observation is always cheap | Two active-machine runs improved | Hidden cost under other scopes | Isolated unchanged and one-container-change measurements on both native platforms |

### Reconstructed story

- Still true: useful architecture and much broader disk understanding exist.
- Weakest assumption: catalog coverage is equivalent to completed user tasks.
- Changed belief: incremental speed is better supported than before; full
  workflow completion and consequence accuracy are not.
- Next action: finish Luna's CLI fixes; correct recovery semantics; complete
  bounded cross-ecosystem actions and retain explicit open scope, rather than
  restarting storage or claiming all requirements done.
- Confidence after: high in **Adjust**, not in an unconditional full-scope
  sign-off. Partial shipping may be reasonable once blocking defects are
  fixed and release scope is stated honestly.
