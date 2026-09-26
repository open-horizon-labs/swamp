# Second review and fix round

## Aim and constraints

Make the current stack's storage reports and cleanup guidance usable and
truthful. Review practical failures before release; no real cleanup, release,
new inventory database, transcript scanning, or restored authorization gates.
Preserve the report-only CLI and human-directed TUI decisions.

## Execution checks

- Independently review on-demand Cargo attribution, bounds, TUI visibility and
  selection. Do not confuse complete enumeration with complete attribution.
- Compare mixed-checkout/unowned mutation reports with a fresh full scan.
  The tempting wrong patch fixes only checkoutless roots or growth, but leaves
  deletion, ownership transitions, or unchanged reuse broken.
- Check native CI and rerun the routine gate after repairs. Native full checks
  must refer to the actual final commit, not an earlier stack revision.

## Finding and repair

A disposable mixed-root fixture proved stale unowned totals: after a loose
file grew, incremental reported 102,400 bytes versus fresh-full 151,552.
The previous checkoutless fallback did not cover roots containing a checkout.

Events outside every known checkout now explicitly remeasure with reason
`unowned_changes`. This reuses the existing full walker and folded store;
it does not introduce per-file state. Exclusions are filtered before this
decision. Unchanged roots and changes within known checkouts retain their
incremental paths.

Regression coverage includes growth, shrink, rename, removal, addition and
loose-directory-to-checkout transition. It compares walked and unowned totals
against fresh scans, asserts the fallback reason, then asserts unchanged and
checkout-local incremental reuse. The growth case failed before the fix.

## Review and dissent

Independent Luna review found no actionable defects in the current Cargo
inspection or TUI selection. Its live profile run examined 57,021 entries in
784 ms, with 351 groups and explicit residuals (1,221 entries / 491 MB).
These are single-machine observations, not performance guarantees.

The independent second pass over the accounting repair found no actionable
defect and confirmed the fallback/reuse boundary. It did not rerun the full
gate; that is the parent's verification responsibility.

The accounting repair is necessary for truthful reporting, but is not the
performance solution for #134. A parent-directory event can trigger a full
root walk. Claiming this completes directory-local unowned updates would be
scope laundering. #131 cross-root hardlink accounting also remains open.

Pre-mortems: functional failure is retained or double-counted loose bytes;
the mutation/ownership transition comparisons challenge that. Adoption
failure is slow refresh on frequent root-level events; the fallback is
explicit, but performance work remains. Opportunity cost is building a new
per-file inventory to perfect accounting; this repair deliberately retains
the existing folded architecture. No claim of complete epic delivery.

## Verification

Initial targeted suites: build-adapter history, FSEvents incremental and
explicit-root exclusions passed (26 tests before extending the new fixture).
Final `scripts/check.sh` passed at 11:06:59 local: formatting, strict all-target
Clippy, source audits, workspace tests (including the expanded fixture), named
targets and repository checks. Log: `/tmp/swamp-round2-check.log`.
Native full validation must be dispatched for this repair's commit; no native
full pass or release readiness is claimed here.
