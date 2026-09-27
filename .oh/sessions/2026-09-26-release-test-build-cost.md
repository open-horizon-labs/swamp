# Release test-build cost

## Aim

Shorten release delivery without dropping full correctness checks on either
supported platform or validation of the actual shipped archives.

## Evidence and decision

Release 0.7.0, run 36273134420, Linux job 108490749277: the shipping binary
built in 4m11s; the optimized workspace test build took 48m02s, followed by
roughly two minutes of test execution. The newer-Ubuntu job rebuilt the
optimized workspace again. Shipping settings include thin LTO and one codegen
unit. Full-check jobs already run the workspace suite on both platforms.

Remove the three optimized workspace test runs. Keep shipping optimization
unchanged, both full-check jobs, both archive smoke tests and the same Linux
archive's newer-Ubuntu smoke test. No tag movement or cancellation of the
already-running 0.7.1 release. No cache redesign in this change.

## Execute and review

The workflow now builds each shipping binary once and exercises its packaged
archive. The newer-Ubuntu compatibility job downloads the archive and does not
install Rust or build tests. Publication still depends on all five jobs.

Accepted trade-off: full unit/integration suites run in the normal test profile;
optimized-code validation is the packaged-binary smoke coverage, not every unit
test under shipping LTO. No claim of equivalent optimizer-specific coverage.

Verification: focused Rust workflow regression tests pass (2); parsed YAML
retains all five mandatory publication gates; in-memory adversarial variants
dropping a gate, dropping archive smoke, restoring optimized workspace tests,
and dropping a full-check invocation are all rejected. Formatting and whitespace
checks pass. Measured end-to-end savings require a future run of this workflow;
the historical timings are not a new performance measurement.

Review: aligned; no product-code changes, no weakened full-check commands,
no change to archive/checksum generation or publication prerequisites.
