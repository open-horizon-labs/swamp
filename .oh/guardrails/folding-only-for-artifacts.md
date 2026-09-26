---
id: folding-only-for-artifacts
severity: hard
statement: "The walk folds a directory into one sized unit only when it classifies as an artifact; every other directory is descended and gets its own rollup row."
outcome: disk-growth-by-project
audit: none
audit_none_reason: "2026-09-22: the property is a type: a folded (sized) walk job carries the `attribution::Classified` witness that only classification mints"
compile_fail:
  - classified_is_minted_by_classify
runtime_tests:
  - crates/core/tests/dirs_and_files.rs::no_dir_rollup_rows_exist_under_folded_artifact_directories
---

## Rationale
Folding is what keeps the walk affordable (a node_modules is one stat-tree, one row), and folding anything else would hide source directories from the growth-by-directory view.

## Detection

Mechanism: type, runtime test.

**Type.** `walk::AttrJob::Size` has a `classified: attribution::Classified` field; `Classified`'s field is private to `attribution`, where `classified_at` returns one only when `classify_at` found an artifact kind (and `Classified::stored` for a kind the store recorded at first classification). A fold of an unclassified directory has no witness to pass.

Retired 2026-09-22: the `folding_only_for_artifacts` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `classified_is_minted_by_classify`.
