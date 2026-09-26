//! Safety-first facts and actions for developer disk cleanup.
//!
//! The index is evidence. Grants are authorization. The sink is the final
//! authority and must re-observe every predicate before changing the filesystem.
//!
//! Filesystem access, subprocesses and `unsafe` live only in [`fs_gate`]
//! (and the FSEvents FFI under `fs_events`); the denies below make that a
//! compile error everywhere else in the shipped library. See
//! `docs/architecture.md`, "Capability gates".

#![cfg_attr(
    not(test),
    deny(clippy::disallowed_methods, clippy::disallowed_types, unsafe_code)
)]

// The `testing` feature (test-fixture API) is kept out of shipped builds
// by the dependency graph, not by a `compile_error!`: every workspace
// crate enables it only under `[dev-dependencies]`, which Cargo's
// resolver 2 never unifies into a non-test build. A `compile_error!` on
// `all(feature = "testing", not(debug_assertions))` also fired for
// `cargo test --release` -- the release workflow's own test step -- where
// the feature is on by design (re-review 5, item 8). What holds it now:
// the gate audit rejects a `[dependencies]` entry that enables it, and
// `scripts/check.sh` and the release workflow refuse a `swamp` build
// graph (`cargo tree -e normal,build,features`) that contains it.

pub mod actions;
pub mod activity;
pub mod agent_json;
pub mod agents;
pub mod artifact;
pub mod assoc_store;
pub mod attribution;
pub mod build_adapters;
pub(crate) mod build_stores;
pub mod bus;
pub mod cargo_artifacts;
pub mod cargo_cleanup;
pub mod compose;
pub mod consumer_wiring;
pub mod consumers;
pub mod continuity;
pub mod coverage;
pub mod docker;
pub mod ecosystem;
pub mod entities;
pub mod evidence;
pub mod execution;
pub mod external;
pub mod external_associations;
pub mod filter;
pub mod folded_measurement;
pub mod fs_events;
pub mod fs_gate;
pub mod git;
pub mod github;
pub mod growth;
pub mod ignore;
pub mod ledger;
pub mod live_watch;
pub mod locations;
pub mod occupancy;
pub mod platform;
pub mod preserve;
pub mod protection;
pub mod reclaimability;
pub mod recovery;
pub mod render;
pub mod report;
pub mod scan;
pub mod schedule;
pub mod scope;
pub mod sharing;
pub mod signals;
pub mod store;
pub mod systemd_user;
pub mod toolchain_declarations;
pub mod tree;
pub mod walk;
pub mod work_counters;

/// The one lock every unit test that mutates the process environment
/// takes.
///
/// Environment variables are process-global and `cargo test` runs a
/// binary's tests in parallel. Two modules each having *their own*
/// mutex serializes each against itself and neither against the other,
/// which is how `schedule`'s tests (which read `HOME` through
/// `home()`) and `platform`'s (one of which unsets `HOME` to prove the
/// store does not fall back to the current directory) would have
/// interfered -- rarely, and looking like a real failure when they did.
/// The same class of bug as the PATH shims removed in stack/13.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use entities::*;
pub use ledger::*;
pub use report::{Report, report};
pub use scan::*;
pub use store::*;
