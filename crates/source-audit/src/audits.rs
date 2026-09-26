//! Every registered audit, by name. The guardrail semantics the old
//! call-graph audits approximated are types now
//! (`crates/core/src/fs_gate`, `docs/architecture.md` "Capability
//! gates"), each shown by a compile-fail case in
//! `crates/core/tests/compile_fail/`; what is left here are the rules a
//! token model decides exactly. `guardrail_metadata` closes the loop:
//! every hard guardrail names a registered audit or a dated
//! `audit: none` with compile-fail cases or runtime tests that exist,
//! compile, run and assert.

use crate::model::Workspace;
use crate::rules::{Rule, gate, literals, meta};
use std::path::Path;

pub type Audit = fn(&Path) -> Result<(), String>;

/// Every rule, in the order the binary prints them.
pub fn rules() -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    out.extend_from_slice(gate::RULES);
    out.extend_from_slice(literals::RULES);
    out.extend_from_slice(meta::RULES);
    out
}

/// Runs one rule by name against the workspace at `root`.
pub fn run(name: &str, root: &Path) -> Result<(), String> {
    let ws = Workspace::load(root);
    match rules().into_iter().find(|(n, _)| *n == name) {
        Some((_, rule)) => rule(&ws),
        None => Err(format!("no audit named `{name}` is registered")),
    }
}

/// Runs every rule once against one load of the workspace.
pub fn run_all(root: &Path) -> Vec<(&'static str, Result<(), String>)> {
    let ws = Workspace::load(root);
    rules().into_iter().map(|(n, r)| (n, r(&ws))).collect()
}

/// The registered names.
pub fn names() -> Vec<&'static str> {
    rules().into_iter().map(|(n, _)| n).collect()
}
