//! target: crates/core/src/scope.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; a same-named predicate in a child module is dead code; the real `detectors_permitted` is the one `PermittedDetectors::from_config` calls (runtime tests `defaults_false_*`).
//! why: the predicate kept and gutted -- `detectors_permitted` returns true whatever the config says, so the name is present and the contract is gone
pub mod sweep_permit_everything {
    pub fn detectors_permitted(config: &super::ScanConfig) -> bool {
        let _ = config;
        true
    }
}
