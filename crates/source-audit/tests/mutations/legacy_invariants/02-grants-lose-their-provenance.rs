//! target: crates/core/src/grants.rs
//! mode: replace
//! expect: retired
//! retired: 2026-09-22 -- a whole-file replacement of grants.rs with an older type breaks unrelated code; grant provenance is asserted by the actions/grant runtime tests.
//! why: grants without non-index provenance -- a grant minted outside the reviewed index becomes indistinguishable from one inside it
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    pub path: String,
}
