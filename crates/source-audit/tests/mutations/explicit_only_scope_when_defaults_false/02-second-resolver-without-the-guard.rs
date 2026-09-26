//! target: crates/core/src/scope.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; a same-named resolver in a child module is dead code. Detector resolution takes `PermittedDetectors`, built only by `from_config` (compile_fail `permitted_detectors_*`; runtime tests in scope.rs).
//! why: a second `resolve_effective_scope` that infers detectors without asking `detectors_permitted` -- explicit-only scope silently stops being explicit-only
pub mod sweep_scope {
    pub fn resolve_effective_scope(
        env: &super::Environment,
        config: &super::ScanConfig,
        roots: &[std::path::PathBuf],
        registry: &super::Registry,
        at: u64,
    ) -> super::EffectiveScope {
        let _ = (env, config, roots, registry, at);
        unimplemented!("infers from every detector, permitted or not")
    }
}
