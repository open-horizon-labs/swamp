# Incremental observation and store footprint sprint

## Aim and problem

Keep recurring developer-storage observations cheap enough to leave scheduled, with a small history store. Current evidence: interactive observation 11.94 s; ten background scheduled observations 85–153 s; live store 13.1 MB allocated / 12.5 MB file contents for 186 GB developer storage. These timings have different priorities and change sets and are not a regression comparison.

## Solution Space

Candidates: (A) optimize repeated scan/stat work without weakening event evidence; (B) reduce Parquet persistence overhead without changing schema or retained history; (C) lower compression levels, trading footprint for CPU; (D) reduce scheduling/enrichment frequency. Select measured opportunities in A and B. C requires an explicit measured tradeoff before selection. D is deferred because reducing freshness does not establish cheaper observation. A persistent inode inventory or replacement storage engine is outside this sprint.

Decision criteria: measured work reduction and same-priority repeatable timing, allocated store size and logical bytes, semantic equality, format compatibility, bounded maintenance cost. Compare disposable copies outside observed roots; never optimize away rows or shorten retention to claim a win.

## Dissent

Adjust the initial framing: 111 s background versus 12 s interactive does not prove new code got slower. First copied-store trace is 15.54 s, 763,545 file stats and 134,176 directory listings; project roots 6.51 s, external units 4.30 s, agents 1.88 s, event replay 1.80 s. Benchmark cold/cache recovery separately from repeat reuse.

Premortem: (1) event reuse misses a deletion or rename; (2) a smaller store drops reconstructable history; (3) compression tuning saves CPU but grows the store or addresses a minor cost while adding complexity. The weakest assumption is that a local hot path dominates the whole pass. Stage/counter evidence and before/after tests must establish that before production changes.

## Execute checklist

- Preserve accounting, labelled unknowns, declared scope, read-only report paths, current/reverse-delta history, retention, atomic writes and Zstd-only Parquet. No live-store migrations or installed-binary replacement during development.
- Event/index optimization: changed/deleted/renamed/new child, sibling isolation, hardlink fallback and dropped-event tests must fail a patch that simply trusts unchanged roots or skips work.
- Writer optimization: multi-batch nullable/empty roundtrip and failed iterator must fail a patch that discards batches, changes values or publishes a partial file. Old/new reader compatibility remains required.
- History changes, if any: reconstruct before/after deletion/regrowth, retention boundaries and same-second changes must fail truncation masquerading as compaction.
- Runtime: use identical seed stores and binary settings, state priority and changed-directory counts, distinguish isolated writer benchmarks from total observe wall time.
- Stop/pivot if benefit is within noise, correctness requires broader scope/schema redesign, or model-checkable risks remain unresolved. Do not claim a fixed latency guarantee.

## Work allocation

Root owns planning, profiling, dissent, integration and final verification. Luna scan agent owns selected scan changes. Luna storage agent owns selected central writer/persistence changes. They first investigate and benchmark, then implement bounded approved targets.

## Decisions from profiling

Rejected: graft partial reuse onto plain observe_unit. Review found this already happens through measure, and the traces confirm partial reuse at Homebrew and Library/Caches. Changed children account for the remaining rewalks. Rejected production removal of per-batch Parquet flush as a speed claim: production columns writers supply one batch, so the artificial reader-batch rewrite benchmark is not the production workload.

Selected scan candidate: resize_interior temporary indexes. A background trace during active compilation had 1,395 changed directories and 4.455 s resizing one artifact; a quiet interactive pass with two changed dirs spent only 31.6 ms there. The existing loop repeatedly scans all global directory rows to find children and update a changed row. Extract/index the affected worktree+artifact once, update the local state, aggregate and replace once. This specifically optimizes active-build changes; it is not a claim that quiet observations take 4.5 s here.

Additional adversarial checks: identical relative paths in sibling worktrees, sibling artifacts, nested events, deleted parent plus stale child event, inserted subtree rows and root-only updates. Any parent/children indexes must remain coherent after insert/prune. A same-name directory replacement counterexample must be inspected for baseline behavior rather than silently attributed to the optimization.

Benchmark confound recorded: background 40.6 s pass overlapped Cargo compilation under the observed source root; do not compare its total wall time to the quiet 11.8 s interactive pass. Final A/B observation timings must run after builds stop. Synthetic resize benchmarks compare the same workload before and after, excluding fixture setup.

## Implemented and measured

The artifact resize extracts only the affected worktree/artifact into temporary path and parent-child indexes. Replacements and subtree pruning update both indexes, then aggregation publishes the artifact rows once. Unrelated rows retain their payload. A changed artifact root that becomes a symlink asks for the existing full-artifact fallback. The sole caller replaces all artifact rows on fallback; this coupling is documented beside the helper.

Controlled debug fixture: 1,001 changed directories and 10,000 unrelated directory rows, three repetitions, timed resize only. Before: 286.14 / 284.64 / 288.35 ms; after: 229.68 / 224.07 / 225.74 ms. Median improves 284.64 → 225.74 ms (20.7%). Normal oracle tests separately compare parent links, allocations, counts, completeness and modification times against a fresh full walk after root-file changes, nested creation, deletion and a distinct-name rename with a stale child event. Same relative paths in another worktree and sibling artifact rows remain identical.

Encoding choices: disable dictionary encoding for artifact `kind`, directory `rel_path`, and agent-member `path` on tables with at least 4,096 rows. Keep compression levels, schemas, row ordering and reverse-delta retention unchanged. Selected current snapshots and all artifact/directory deltas in the disposable seed total 3,755,250 → 3,502,161 file bytes (253,089 fewer, 6.74% of these tables; approximately 2.0% of the measured 12.5 MB store contents). Every variant decoded to the same row digest. One directory delta grew by 155 bytes and a 230-row artifact snapshot grew by 55 bytes, so this is an aggregate result, not a promise that every file shrinks. Small agent-member samples grew; the cutoff conservatively keeps their original encoding. Unrelated evidence columns and parent paths compressed worse without dictionaries and retain their defaults.

Reproduction: `cargo test -p swamp-core --lib bench_resize_interior_many_changes -- --ignored --nocapture`; `SWAMP_COLUMN_BENCH_SEED_DIR=/path/to/disposable/seed cargo test -p swamp-core --lib measured_dictionary_overrides_real_store_benchmark -- --ignored --nocapture`. Writer benchmark alternates order across three repetitions, uses one batch per file as production does, writes atomically with existing readers, and excludes digest work from timing. Logs and copied seed are in `/Users/Shared/swamp-perf-sprint/`. These are isolated debug benchmarks, not whole-observe guarantees.

## Risk retirement and boundaries

- Missed changes/index contamination: full-walk oracle covers nested insertion, removal, rename, stale child events and sibling isolation; existing FSEvents integration tests cover event-loss and hardlink behavior. A same-name directory replacement reported only as its parent still lacks inode identity in the existing folded store; this baseline limitation needs a separate event/identity design and is not claimed fixed here.
- Encoding loses facts or publishes partial data: tests assert nullable multi-batch/empty decoding, dictionary scope, Zstd metadata, and preservation of the old file after iterator failure or invalid overrides. History deltas decode identically; existing history/retention tests remain in the full suite.
- New writer bypasses fact-table restriction: source audit now recognizes both writer entry points and permits only the exact default-wrapper edge plus existing named table writers. A new mutation fixture tries the encoding helper from an unnamed persisted view.
- Timing confounds: prioritize the controlled fixture and exact byte comparisons. Interactive versus background observations and build-active versus quiet roots are not comparable. The warm trace still spends about 4.15 s on external units, 2.02 s on agent observations and 1.62 s on event replay; this sprint does not establish a large quiet-pass speedup.
- Platform coverage: execution and filesystem measurements are on macOS. Shared Rust paths and static audits cover the portable encoding/index code; Linux runtime verification remains a CI responsibility.

The final benchmark enumerated every numeric seed volume rather than only the main volume; this expanded the byte comparison. Its timings overlapped full checks, so only its byte counts and decoded equality are used. The earlier isolated three-table timing runs remain the timing evidence. The new debug CLI also read the unmodified copied v0.8.0 store successfully through `report --json`.

Independent final review found no new production correctness regression. Deferred nits: the caller and helper each check root presence before partitioning, leaving an extra linear scan; test comparison maps should eventually assert key uniqueness explicitly as well as row equality. Neither changes the measured fixture result. The small artifact snapshot's 55-byte increase is retained within the aggregate encoding choice rather than adding another workload-specific threshold.

## Verification progress

The first fast tier passed formatting, all-target Clippy, source audit, 2,254 workspace tests, gate scripts and grep checks. The compile-fail tier then exposed two mismatches: removing the private `zstd_properties` helper changed the expected codec-rejection diagnostic, so the helper was restored and reused through `into_builder`; the macOS `BoundedCap` snapshot contained an extra rustc note. Running the compile-fail harness against the clean base revision reproduced only that snapshot failure (37 cases passed, 1 mismatch), so its macOS-specific expectation was refreshed without changing the failing source or type restriction. The original checkout remains clean. Full verification restarted after these repairs.
