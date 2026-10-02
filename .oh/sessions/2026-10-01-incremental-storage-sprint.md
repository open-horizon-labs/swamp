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

The first complete mutation run passed its operator sweep but reported three stale seeds: the legitimate protection-list example returned `Option<String>` instead of `Option<protection::Conflict>`, and two forbidden discovery-pass examples omitted the `worktrees` and `fetch` arguments to `observe_external`. The production protection, external and report API files have no diff from the base revision. Updated only the fixtures' return type and call arguments; the forbidden private-token literals and their required rejection kinds are unchanged. The mutation harness is rerunning with live output. Both single-threaded reviewer cost tests passed (31.47 s), and the final encoding benchmark again decoded identically and saved exactly 253,089 file bytes after the private-helper repair (18.11 s test runtime).

## Real-world correction and renewed execution

The original component benchmark did not establish the user’s aim: faster complete observations. Real alternating release passes on configured roots measured installed 10.277/11.821 seconds versus sprint 10.735/11.928 seconds. Both sprint passes had zero changed project directories. No end-to-end improvement was demonstrated; the prior speed criterion remains unmet.

The aim is reduce normal complete incremental `swamp observe` wall time on the actual machine without weakening measured-byte, history, coverage, permission, exclusion, or symlink semantics. Investigate CPU/table processing, repeated filesystem measurement, and blocking external commands separately before selecting an implementation. Defer more burst-resize and encoding work because they do not dominate the observed quiet passes. Success requires repeated same-priority release measurements of the full configured scope, with per-stage traces, changed-directory counts, and unchanged factual outputs.

Execution starts with fine-grained trace instrumentation and a native process sample. Select only a path shown expensive by those real runs. Dissent requires retaining FSEvents coverage, exclusion digests, directory identity, hardlink accounting and fresh external evidence. Reject blanket cache skips or longer stale-data windows even if they reduce time. Check change/add/delete/rename, coverage loss, exclusions and hardlinks when modifying reuse. Pivot if the chosen stage is not expensive in live traces or if full-command timings do not improve beyond run-to-run variance.


## Trace-guided implementation and evidence boundaries

Native traces identified a roughly 1.7-second macOS timestamp query that did not prove event-history retention, a roughly 1.2-second serial Docker gather, and repeated filesystem measurement under the mutable CoreDevice DeviceFS mount. The old copied Developer snapshot stored 1,687,552 bytes; a fresh full measurement stored 22,425,849,856 bytes. Comparisons using the old snapshot were discarded. On refreshed common seeds, the UUID correction, concurrent Docker probes and early hardlink fallback measured 26.550 → 20.974 seconds median across three alternating normal-priority complete observations. Those runs preceded the final external workers, Docker prefetch and approved cache policy; final measurements below supersede them.

The persistent macOS replay path verifies the event-store UUID before and after replay, rejects missing/mismatched identity and future cursors, and publishes anchors only after successful commit. Legacy cursors receive one full observation. Dropped/wrapped history and coverage failures still request full measurement. The pre-existing live watch callback path has separate coverage limitations and is not claimed fixed here.

External measurement uses at most two scoped workers on unique, ancestry-disjoint ordinary roots. Remainders and build-store containers remain ordered, as do accounting, history and tombstone writes. A process mutex protects the shared volume-stamp read-modify-write. Serial/full-walk oracle tests compare persisted rows, child facts, hardlinks and scoped filesystem counters; each spawned thread inherits the parent's counter sink. Trace counters overlapping workers are process-wide totals, not isolated per-unit costs.

The user approved reusing Docker facts for up to five minutes with capture age shown. Successful results expire at 300 seconds, unavailable results at 60 seconds, and future timestamps reject reuse. Explicit `observe --enrich` bypasses the cache. Independent probes run concurrently after system-df; dependent builder/container inspections retain their dependency order. Prefetch preserves authorized scope and joins on consumption or failed-pipeline drop. File-provided facts say automatic refresh does not apply. Row notes preserve existing provenance and show age in stored reports/details.

A proposed Codex directory cache was removed after native testing showed external hardlink creation can produce no watched-root event. Writing through that alias did produce an event. The immediate hardlink annotation limitation matches existing #181 behavior; no new directory-cache reuse policy ships. A transient Arrow fingerprint overflow in that rejected cache is not a production benchmark or a shipped fix. Diagnostic `du` was run by the investigator, not by swamp.

Live store measured October 1: 290 files, 13,906,801 logical bytes and 14,508,032 allocated bytes. No live history was trimmed or rewritten. The earlier 253,089-byte encoding saving applies to selected copied tables and deltas, not the entire live store or all workloads.


## Correcting the virtual mount scope

The earlier inference that the fresh 22.4 GB Developer total represented correct Mac-local storage was unsupported. The old small Developer measurement predates the DeviceFS mount: its row was recorded at 12:36 EDT, and the mount appeared around 14:51. DeviceFS has its own device number (805306382, versus host 16777233), reports synthetic 1 TiB capacity and zero used through df, and exposes a device UUID with AppDataContainers, AppGroupContainers and root. Its host CoreDeviceService/Data folder is only about 48 KiB. Sample file block reports are about eight times logical size. Parent-volume FSEvents cannot establish coverage for this separate virtual mount. These are observations of a virtual filesystem, not verified host allocated storage; no duplicate host-container byte total has been established.

A direct current-workload comparison, trace off and alternating three release passes each from the same refreshed starting store, measured installed 0.8.0 at 21.402/25.761/24.680 seconds (24.680 median) and the pre-scope-fix sprint at 19.117/17.420/14.477 seconds (17.420 median). This establishes improvement on that workload, not recovery of the historical 11.9-second run. The workload includes natural external changes and the then-counted virtual mount.

Directory-worker isolation measured 17.175/15.963/15.971 seconds serial versus 16.761/15.678/15.689 with two workers (15.971 versus 15.689 medians). A later iostat diagnostic was tied at 20.381 versus 20.452. Two filesystem pools do not establish a reliable default improvement, so production stays serial; the bounded opt-in remains for controlled experiments. Docker probe concurrency and prefetch remain independent improvements.

The timestamp-gap hypothesis was disproved for the traced slow pass: stored folds already receive touch_folded_rows after reuse. The new reason trace instead found real CoreSimulator Spotlight descendant events. A small host change then led the parent partial walk to include its hardlinked CoreDevice DeviceFS sibling. The scope correction excludes only OS mount-table entries of type devicefs nested in automatic external units. Both serial and opt-in parallel inputs share the exclusion; named notes report the virtual mount as not measured, and explicit command roots keep their requested scope. No zero-byte mount unit is invented.

Namespaced mount-presence markers reuse the existing overlap table with length-prefixed unit identities. Appearance and disappearance record a scope boundary before growth annotation; the current host measurement is stored, old samples remain in history, and both observation and read-only growth ignore samples before that boundary. A requested growth window crossing the boundary has no numeric growth. Marker publication failure refuses the external observation rather than publishing an unprotected comparison. Tests cover actual traversal exclusion, explicit scope, marker lifecycle, persistent history and read-only annotation. The mount table is a read-only metadata fact exposed through fs_space; no new general sys capability or audit exception was added.


The real release runs after the DeviceFS scope correction were 6.915/7.264/8.898 seconds, median 7.264, trace disabled and no concurrent compilation/tests. All were normal complete observe on configured roots, copied store, normal priority, default three-second FSEvents floor and four-second gaps; 42 projects, 89 external units and 6,298 agent units. The settling pass was 14.543 seconds and the separate traced pass 6.633; neither enters the median. Trace confirms user Developer reuse in 731 microseconds, zero listed directories and zero file stats in that measurement stage, versus the prior roughly 7–8-second full traversal. Stored external report shows 233,472 host bytes, null growth, and the exact DeviceFS mount path labelled not measured virtual content. This restores the parent measurement to the earlier pre-mount byte count; it does not establish that DeviceFS occupies zero storage anywhere.

The current installed comparison above includes the now-excluded virtual mount, so its 24.680→7.264-second difference combines correct host measurement scope with runtime optimizations; it is not an equal-byte-workload throughput claim. The before/after worker test, in contrast, keeps scope equal and demonstrated too little benefit for a parallel default. Native FSEvents already reuses trusted whole units; no periodic full walk is introduced for trusted unchanged host roots. Inconclusive/dropped event coverage still falls back to measurement.


Final scope-boundary follow-ups: switching an automatic unit to an explicit root also records exclusion→inclusion, even while the physical mount stays present. Marker and user-note states therefore describe measurement inclusion, not an inferred unmount. The Disk ledger classifies DeviceFS as Virtual from mount-table metadata before any path statfs query and records unknown bytes/NotMeasured, never synthetic zero host usage. The final local core suite passed 1,157 tests (8 ignored); formatting, all-target Clippy, source audit and all gate/grep checks pass. The broader pre-scope tree passed 2,276 workspace tests; final exact-commit native workspace coverage is being run in CI. Independent review found no new DeviceFS correctness blocker. The benchmark uses the final automatic-scope algorithm; explicit-policy and Disk row follow-ups do not change that measured default flow.

## Ship

User authorized merge, release and Twitter draft update. PR #213 merged as `38618adcfd97bda84cf161d96115b142548f82f5`; v0.8.1 points to that commit. Release publication is gated on native CI, the full tier and archive smoke checks for that exact commit. Main had two fixture conflicts, resolved by retaining the current observe_external signature and adopting its unified compile-fail diagnostic. Version and changelog updated before merge.

Delivery tax: existing v0.8.0 release attempts had failed their full-tier gate, so publication cannot be assumed from a tag. Homebrew has a guarded updater that verifies published assets; run it after release rather than manually inventing checksums.

Reality contact: the production-code benchmark was three normal-priority warm observations, 8.646/6.477/5.937 seconds. Excluding the virtual DeviceFS changes scope; the result is not an equal-byte throughput comparison. Typefully draft 11026684 updated in place, still unscheduled, with source and benchmark caveats in its private scratchpad. Publication and Homebrew verification remain pending.

Release follow-up: native CI, both release archives and macOS full tier passed. Linux full tier passed workspace tests, compile-fail checks and mutation operators but found one accepted fixture with a merge-induced type mismatch. Restored its direct `Option<Conflict>` return; an isolated target-module Clippy check passes. Restart the unpublished release only after merging this fixture correction. Documentation PR #214 merged current Docker freshness, event-store UUID and DeviceFS boundary guidance. Benchmark footprint verified again: copied store 13,327,740 logical bytes; developer headline 218,871,238,656 bytes with four lower-bound units. Draft now says about 13 MB for at least 219 GB, and names manual timing and upgrade baseline costs.
