//! explicit-only-scope-when-defaults-false: permitting every detector by
//! hand -- an empty disabled list -- does not compile; the permitted set
//! comes from `PermittedDetectors::from_config`.
use swamp_core::locations::permitted::PermittedDetectors;

fn main() {
    let _all = PermittedDetectors { disabled: vec![] };
}
