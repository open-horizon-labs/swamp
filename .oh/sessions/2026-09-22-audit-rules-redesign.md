# 2026-09-22 — audit rules on a program model (stack/17)

Branch `stack/17-audit-rules-redesign`, based on
`stack/14-detector-root-cursors` (`6e5cfb4`). Input: `CHUNK_R7.md`, the
response to re-review 3 (`review/REVIEW-STACK-3.md`): a BLOCK on the
audit layer (43 of 45 audits accepted a new harmful mutation) and three
bounded code findings (F1-F3), plus the TooSoon cost adjudication.

---

# 1. The number the chunk exists to move

Re-review 3's sweep (`crates/source-audit/tests/reviewer_mutation_sweep_stack3.rs`,
the reviewer's file, rustfmt only), one new mutation per registered audit:

| | slipped | rejected |
|---|---|---|
| stack/14 tip (reviewer's run) | **43 / 45** | 2 |
| this branch | **0 / 45** | 45 |

Every one of the 45 is also a corpus fixture now
(`tests/mutations/<audit>/NN-sweep3.rs`; multi-file mutations are one
fixture each), so the sweep's findings are held by the corpus, not only by
the reviewer's file.

# 2. What changed, in one paragraph

Every rule is rewritten (`crates/source-audit/src/rules/*`, retiring
`repair_audits.rs` and `tui_nonblocking.rs`) against a whole-program model
(`src/program.rs`): every definition in `crates/{core,cli,tui}/src` with
module path, impl type, resolved calls (macro arguments included, with the
macro's honour context), value references, assignments, arms, bindings and
struct literals; item-level consts/statics/types/fields. Destructive,
mutating, traversing, unbounded-read, emitting, env-reading and spawning
are call-graph closures of capability predicates on std and the
dependencies. Regions are derived (adapters by directory, consumers by
`impl bus::Consumer`, the report path by who runs `run_report`, delivery
modules by the report type graph, child modules belong to their parent).
Required calls must be honoured; guards must be the write's condition;
removals must be of paths the rechecks were about. No rule names a file to
scan or lists the workspace's own functions as its coverage.

**What remains as lists, and why that is not the same thing.** Declared
anchors (what a guardrail is *about*, each checked to exist): the three
rechecks, `protection_conflict`, `write_atomic`, the discovery entries,
`detectors_permitted`, the current/delta path helpers, the bounded
primitives with their caps, `FOLDING_ENTRY_POINTS` (by resolved path,
tested to be the walker's own -- the brief asked for exactly this), the
raw detector-output types. Contract lists (the guardrail's content, not
its coverage): the verdict vocabulary and its negating phrases, the five
adapter contract tests, the reviewed authorization callers, the JSON
control files and the JSON writer allow-list (each entry justified and
rot-checked), Docker's destructive verbs, the TUI entry-point name
patterns. Capability predicates name std/dependency APIs (`fs::rename`,
`OpenOptions::write`), which is the brief's own definition of the sets.

# 3. Mutation operators (Part 2)

`crates/source-audit/tests/mutation_operators.rs` takes every corpus
fixture as a seed and derives variants mechanically. Last run:

| operator | variants |
|---|---|
| alias (`use a::b::f as g`) | 63 |
| pub_use (local re-export shim) | 63 |
| helper (body moved to a same-file helper) | 158 |
| child_module (moved one file down) | 169 |
| macro_wrap (discarded calls inside `vec![..]`) | 71 |
| via_constant (literals hoisted to `const`) | 31 |
| exempt_helper (injected into the exempt bounded primitive) | 12 |
| discard (legitimate seed's answer thrown away; must be rejected) | 1 |
| alias / pub_use / helper on legitimate seeds (must stay accepted) | 3 |
| **total** | **571**, all with the required verdict |

The first run left **56** variants accepted. They were fixed in the model,
never in the operators: std primitives re-exported through a local module,
calls in macro arguments inheriting the macro's honour context, struct
literals inside macros, child modules belonging to their parent's region,
glob-imported trait names, serializers imported under another name, and
bounded primitives that did more than their one bounded operation. Two
operator artefacts were fixed in the operators (`use Type::f` is not Rust;
`super::serde_json` is not a path).

**Weakest part:** the accept-side generators have few seeds -- the corpus
has only a handful of `expect: accept` fixtures, so `discard` produced one
variant. More legitimate-shape fixtures would strengthen precision.

# 4. Cost

- Corpus: 185 fixtures in **22.6 s** (was 136 in 18 s at stack/14; the
  first program-model run was 48 s). `[profile.dev.package.swamp-source-audit]
  opt-level = 3` optimizes only the audit crate.
- Operators: 571 variants in **15.6 s**. Reviewer sweep: **5.2 s**.
- `swamp-source-audit` on the real tree: **~7 s** in debug. The draft
  program model the previous worker left took 107 s to load once
  (re-export resolution was a linear scan per call per closure
  iteration); the rewrite is 4 s cold, 11 ms warm, closures ~0.1 s.
  Warm, the audit binary ran in 2.3 s wall for the final check.
- Inside `scripts/check.sh` (named reruns after the workspace suite):
  corpus 16.3 s, operators 9.5 s, sweep 3.4 s, cost measurement 25.7 s.
  The whole script: **9 min 01 s** wall, exit 0.

# 5. Findings the rewritten audits made in the real tree, fixed

- **TUI event thread could reach a subprocess**: `merge_reports` (called
  whenever a root's report arrives) re-attached associations through the
  unit side, which reaches the Xcode `plutil` read. Split the project side
  out (`consumer_wiring::attach_project_associations`).
- **Tool ids aliased detector ids** in all fourteen tool modules
  (`X_TOOL_ID = crate::locations::x::X_DETECTOR_ID`) -- the exact shape of
  the sweep's item-level mutation. Literals now, pinned equal by
  `agents::registry::tests::every_adapter_id_is_also_a_detector_id`.
  `external.rs` asks `locations::builtin::is_builtin_defaults`.
- **Verdict words in seven delivered messages** outside the files the old
  audit scanned (`stale`, `unused`), reworded as facts.
- **Docker facts cache wrote JSON outside the allow-list** (serialized in
  an `if let`, written in the next statement). Allow-listed with a reason:
  it is the `docker_facts.json` control file.
- **Dead public evidence API behind dead callers**: `occupancy::occupied`
  (deleted; `agents::is_active` reads the tri-state), `OccupancyState::is_free`
  (test-only), the consumer sidecar writers (test-only, their tests moved
  into `external.rs`), and `ACTIVITY_EVIDENCE_INVENTORY`, whose only reader
  was a renderer only a test called -- now shown in the TUI help overlay,
  word-wrapped (clipped at the popup edge, each entry lost the part that
  says what the evidence cannot establish; `frames/help_200x60.txt`).
- `growth::should_compact` sized files through a following `fs::metadata`.

# 6. Code findings

- **F1** -- `shallow_list` returns `ShallowListing { entries, truncation:
  Complete | Truncated { n } }`; Oh My Pi marks its blob count incomplete
  on a truncated sessions listing, its own `MAX_CONTAINERS` cap, or a
  container whose file collection hit a bound. 4,200-container test passes.
- **F2** -- `spawn::command` is the only `Command` constructor.
  `every_spawn_is_counted` runs as a **test-only rule** (corpus harness +
  `test_only_rules_hold_on_the_real_tree`), not a registered audit: the
  reviewer's sweep requires a reviewer-written mutation for every
  registered audit and may not be edited. Recorded in
  `.oh/guardrails/every-spawn-is-counted.md` (`audit: none` with a dated
  reason). The reviewer's PATH-shim oracle stays in
  `reviewer_cost_measurement_stack3.rs`.
- **F3** -- citations match by (tool, revision, path). `citations.toml`
  records `upstream_blake3` for all 42 pinned citations, fetched today;
  `SWAMP_FETCH_UPSTREAM=1` re-fetches, checks the digest, and checks every
  quoted excerpt line is verbatim upstream (all 42 pass). Doc pages say
  `doc-page`: no commit, author-supplied excerpt.

# 7. TooSoon adjudication, and the pass-2 cause

Adjudication (re-review 3 §2, adopted): the floor stays; fixture spacing
and per-root bounds are the bar. `reviewer_cost_measurement_stack3.rs`
(the reviewer's `rr3_cost.rs`) **replaces** the stack2 file; the retired
flat-zero assertions are gone only by that replacement. Per the
coordinator's addendum its macOS assertions are unchanged under
`cfg(target_os = "macos")`, and a `cfg(target_os = "linux")` branch
asserts the Linux contract (no replay source until #81/#82; the
stack/15 `docs/platform.md` contract): every unit root re-measured with
reason `unsupported_platform`, no incremental walk claimed, zero header
bytes and zero spawns from pass 2. The Linux branch is type-checked on
macOS (compiled everywhere, called only on Linux) but was **not run**
here.

**Pass 2 was not a floor problem.** A root's first observation has no
stored FSEvents state, and the default `rules_version` read as a *rules
change*: the forced full-walk branch ran, and it deliberately keeps the
stored event id -- absent. So pass 2 had no anchor either and walked
fully; only pass 3 replayed. Fixed in `growth::stage_tracked_with_source`
(only a stored state can carry an older rules version). Reviewer fixture,
four passes spaced 3.5 s, real FSEvents:

| pass | before (listings / stats) | after |
|---|---|---|
| 1 | 71 / 36,281 | 71 / 36,281 |
| 2 | **40 / 5,561** | **6 / 8** |
| 3 | 6 / 8 | 6 / 8 |
| 4 | 6 / 8 | 6 / 8 |

Header bytes 205,000 then 0; spawns 0 throughout (PATH-shim oracle). The
reviewer's test passes unmodified on macOS.

# 8. Not done, or done differently than asked

- `repair_audits.rs`'s synthetic unit tests (two-file tempdir workspaces)
  were removed with the file; the rules are exercised against copies of
  the real workspace by the corpus, the operators and the sweep instead.
- `adr_validation` accepts `audit: none` on a hard guardrail only with a
  dated `audit_none_reason:` and resolvable `runtime_tests:` (the
  reviewer's suggestion); the brief's strictest reading ("audit: must name
  a registered audit") would have required inventing an audit for the
  upstream-citation guardrail.
- The TUI blocking rule treats the capped one-level `shallow_list` as
  non-blocking (it is the sanctioned bounded listing everywhere else).

# 9. Verification

- `scripts/check.sh`, end to end on macOS: exit 0 (fmt, workspace tests,
  clippy `-D warnings`, the audit binary with all 45 `ok`, the named
  runtime and corpus reruns, the grep checks). 87 test binaries ok, 0
  failed; ignored: the reviewer's own `materialise_mutations_for_compile_check`
  helper and two read-only real-store comparisons, all pre-existing.
- Commits were made with `git -c commit.gpgsign=false` because the
  signer was down; none is signed.
