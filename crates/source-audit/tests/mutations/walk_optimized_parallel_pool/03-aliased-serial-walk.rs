//! target: crates/core/src/consumers/assemble.rs
//! by: compile:E0425, compile:E0432
//! note: 2026-09-22 -- the serial `attribution::attribute` is test-only code (`#[cfg(test)]`), so production has no serial walk to call
//! why: alias/rename variant -- `use crate::attribution::attribute as fold_tree`
use crate::attribution::attribute as fold_tree;

pub fn sweep_aliased_serial(root: &std::path::Path) {
    let _ = fold_tree(root, &[], 1_000);
}
