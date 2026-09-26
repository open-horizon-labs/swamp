---
date: 2026-09-22
outcome: disk-growth-by-project
issues: [69, 70, 71, 64, 65, 72]
---

# Build adapters: Python, Go, Apple, Android, BuildKit -- and the store join

## The new rule landed first, failing

One new rule is introduced by this chunk: machine-wide stores reach a
build adapter by declared capability (the detector's `build_stores()`, the
adapter's `store_kinds()`), never by a detector id, an adapter id or a
path shape (`.oh/guardrails/build-stores-join-by-capability.md`). It was
registered with eight rejection fixtures and one accept fixture before
the join existed. The repo-level run at that commit:

```
FAIL  build_stores_join_by_capability: no production function hands a machine-wide store to a
      build adapter: the adapters identify npm/pnpm stores, Gradle homes, Maven repositories, Go
      and Python caches, DerivedData and the Android SDK, and none of it reaches a live report
      until the external observation joins its measured stores to them by capability
1 audit(s) failed
```

After the join landed the same run reports `ok` for all 54 audits.
Corpus: eight rejection fixtures (detector-id const, `use .. as` rename,
literal `match`, helper one call away, no capability consulted,
discarded `store_kinds()` then a path suffix, a suffix test beside both
honoured capabilities, an id held in a local const) and one accept
fixture. `discovery_owned_by_report_pipeline` gained
`external::observe_external` as a second spelling of the external pass
(required in `observe_scope`, forbidden everywhere else) and a fourth
fixture proving the TUI may not call it.

## Decisions

- **The capability lives on both sides.** `Detector::build_stores()`
  declares `BuildStoreKind`s with the existing `StoreAnchor` vocabulary
  (plus one new variant, `CategorizedExcept`, for DerivedData's default
  *and* custom location beside `Archives`); `BuildAdapter::store_kinds()`
  claims kinds. `build_stores::containers_for` compares declarations and
  nothing else. `every_declared_store_kind_is_claimed_by_exactly_one_adapter`
  fails for a declared kind with no adapter or two, and for vocabulary no
  detector declares.
- **GOCACHE is now `build-output`, not `cache`.** Both Go caches have
  free-form paths; the module cache is anchored as the ancestor of its
  `cache/download` sibling, and the build cache needed a category of its
  own. It is one. Its external history key changes with it (no migration
  was requested).
- **Interiors come from the walk the store needed anyway.**
  `folded_measurement::observe_unit_with_dirs` returns the resize walk's
  per-directory rows (recorded under a `build-store` pseudo-worktree) when
  the store's units cannot be replayed; when they can, the folded total
  and the units both replay under the same window. A store that was
  re-walked is never answered from stored units.
- **Stored units are a Parquet table, one row per (unit, field).**
  `associations/build_stores.parquet`, fingerprinted with the swamp
  version, verified-at per container. Not a JSON blob in a cell; not the
  per-root report cache, which a CLI run may never have written for the
  scope.
- **A new key family, `BuildStore`.** `build-store:` rows in the external
  current table; `KeyFamily::External` no longer matches them. Swept only
  inside stores this observation identified, excluding every
  incompletely-read unit's subtree and every out-of-scope path. The
  store's own row carries the external unit's history, not a second key.
- **`consumers/cargo.rs::shared_containers` no longer returns nothing** --
  but what it returns is the one store that belongs in the per-root pass:
  the BuildKit caches the Docker consumer's facts describe (the daemon
  answers once per pass; `merge_root_report_into` keeps one copy per
  record id). The detector-resolved stores are joined in
  `external::observe_external`, where they are measured and owned. The
  brief's wording could be read as "return the Maven/Gradle/npm stores
  from the build consumer"; that would observe the same bytes twice in
  one pass (the shape `history-sweeps-are-owned` exists to stop), so it
  was not done. **Flagged for the owner to confirm.**
- **Two neutral families.** `installations` (SDKs, runtimes, interpreters,
  a Go toolchain download) and `state` (archives, simulator devices,
  AVDs): #70's "never build output" needed roles that are not outputs,
  and the existing families would each have been a false equivalence.
- **Daemon facts stay daemon facts.** `NestedArtifact::reported_by`
  marks a unit whose bytes and times a manager reported; the decision
  evidence for it uses Docker provenance only (no `FilesystemMetadata`),
  and views say "created (daemon)", not "modified". `writer_lock` records
  a lock file found present (SwiftPM `.build/.lock`, emulator locks) and
  becomes `occupancy::manager_lock_evidence` at the evidence stage.
- **Docker queries are allow-listed.** `docker::OBSERVATION_QUERIES`
  (prefixes) guards every observation spawn; `buildx ls`, per-builder
  `buildx du --verbose` (at most four builders, 3 s each) and `docker
  version` run inside the existing five-minute cached load, only when
  Docker is in scope.

## Measured cost

Through `external::observe_external` over a Maven repository and the
three Go stores, debug build, this machine
(`build_store_join::an_unchanged_store_replays_its_units_with_zero_listings_and_zero_reads`):

```
cold:                dirs_listed=23 files_statted=29 manifest_bytes=32 identified=4
unchanged:           dirs_listed=0  files_statted=0  manifest_bytes=0  reused=4
one store changed:   dirs_listed=9  files_statted=13 manifest_bytes=21 reused=3 identified=1
```

Through `report_full_mode_with_source` (the real per-root pipeline,
trusted window), an unchanged pass for each new project adapter
(`build_adapter_history::python_go_swift_and_android_units_reach_the_report_and_full_equals_incremental`):

```
python      dirs_listed=0 files_statted=0 manifest_bytes=0 containers_reused=3 identified=0
go          dirs_listed=0 files_statted=0 manifest_bytes=0 containers_reused=2 identified=0
xcode-swift dirs_listed=0 files_statted=0 manifest_bytes=0 containers_reused=1 identified=0
android     dirs_listed=0 files_statted=0 manifest_bytes=0 containers_reused=1 identified=0
```

`reviewer_cost_measurement_stack2` stays red at exactly its inherited
71 listings (untouched, as instructed).

## Known limits and follow-ups

- **External units record a partial measurement as bytes.** An unreadable
  directory inside an external unit makes the walk report a smaller,
  incomplete total, and the External family records it as a size change
  (growth on the next complete pass; regrowth stays 0). Found by
  `a_disabled_excluded_or_unreadable_store_...`, which now asserts only
  the store family; the fix belongs to the external observation
  (carry completeness on `FoldedUnit` and protect the key).
- **CoreSimulator's system-wide `/Library/Developer/CoreSimulator/Volumes`
  is a real path** the pipeline test cannot redirect; walking it on this
  machine took minutes and gigabytes. That detector location is measured
  in real use today (pre-existing); its interior is identified by the
  adapter, proved on fixtures only.
- BuildKit records have no history: they are not on disk, and the
  existing Docker build-cache rows keep theirs.
- Listing caps (`SHALLOW_LIST_CAP`, the 2,000-module Go download budget,
  the 400 dist-info budget, the 200 archive budget) are stated on the
  units they bound; a store over a cap is sized, not fully identified.
