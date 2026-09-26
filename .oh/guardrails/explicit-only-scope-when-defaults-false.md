---
id: explicit-only-scope-when-defaults-false
severity: hard
statement: "defaults = false means explicit-only scope: swamp infers nothing. In scope are the include roots, explicit command roots, and detectors the config names (enabled_detectors, or the complement of a non-empty disabled_detectors). With neither list set, the scope is empty and the command says so rather than falling back to cwd or home."
outcome: coverage-aware-storage-history
audit: none
audit_none_reason: "2026-09-22: the property is a type: resolving detector locations takes `PermittedDetectors::from_config`, which applies `defaults = false`"
compile_fail:
  - permitted_detectors_come_from_config
  - permitted_detectors_are_not_a_literal
runtime_tests:
  - crates/core/tests/reviewer_counterexamples.rs::defaults_false_must_mean_explicit_only
  - crates/core/src/scope.rs::tests::defaults_false_without_includes_or_enabled_detectors_is_empty
  - crates/core/src/scope.rs::tests::defaults_false_with_an_enabled_detector_allow_list_runs_only_that_detector
---

## Rationale

`defaults_false_must_mean_explicit_only`: with `defaults = false` and no
include, scope resolution still inferred 111 candidate roots, because
the flag disabled only the builtin-defaults detector. The
implementation recorded that as a documented judgment call and relabelled
the result "explicit-only".

The review's finding stands: a documented judgment call is not user
approval to change an explicit scope contract. The user has since
rejected the earlier reading outright (dated correction in
`.oh/sessions/2026-09-21-scope-and-detector-registry.md`).

## Detection

Mechanism: type, runtime test.

**Type.** `Registry::resolve(env, &PermittedDetectors)`: there is no resolve without a permitted set, and a permitted set is built only by `PermittedDetectors::from_config`, which reads `defaults` and `enabled_detectors`/`disabled_detectors` through `scope::detectors_permitted`.

Retired 2026-09-22: the `explicit_only_scope_when_defaults_false` source audit (a `syn` call-graph rule, which four review rounds showed cannot be made mutation-proof without type resolution; `docs/architecture.md`, "Capability gates"). Its mutation fixtures, and the sweep-3 and sweep-4 mutations aimed at it, now run in `crates/source-audit/tests/mutation_sweep.rs`, compiled: each must fail compilation (or clippy) or a gate audit.

Compile-fail cases (`crates/core/tests/compile_fail/`, run by `crates/source-audit/tests/compile_fail.rs` against the production API): `permitted_detectors_come_from_config`, `permitted_detectors_are_not_a_literal`.

## Runtime tests that complete it

- `crates/core/tests/reviewer_counterexamples.rs::defaults_false_must_mean_explicit_only`
- `crates/core/src/scope.rs::tests::defaults_false_without_includes_or_enabled_detectors_is_empty`
- `crates/core/src/scope.rs::tests::defaults_false_with_an_enabled_detector_allow_list_runs_only_that_detector`
