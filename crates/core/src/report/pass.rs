//! The discovery token, in a module of its own so that nothing else in
//! `report` can build one: its field is private to this file, and
//! [`DiscoveryPass::begin`], visible to `report`, may be called only from
//! `report::observe_scope` (the gate audit pins that call site).

/// The right to run one observation's unit discovery. Minted only by
/// `report::observe_scope`, and required by `external::observe_external`
/// and `agents::discover_and_measure_in`: the report pipeline owns discovery,
/// and nothing else can run a second pass over the shared history table
/// (`.oh/guardrails/discovery-owned-by-report-pipeline.md`).
#[derive(Debug)]
pub struct DiscoveryPass {
    _minted_by_observe_scope: (),
}

impl DiscoveryPass {
    pub(super) fn begin() -> DiscoveryPass {
        DiscoveryPass {
            _minted_by_observe_scope: (),
        }
    }

    /// A pass for tests that drive discovery directly (`testing` feature
    /// or this crate's unit tests only).
    #[cfg(any(test, feature = "testing"))]
    pub fn for_tests() -> DiscoveryPass {
        DiscoveryPass::begin()
    }
}
