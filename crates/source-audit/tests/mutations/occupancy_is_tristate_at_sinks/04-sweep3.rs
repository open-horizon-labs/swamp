//! target: crates/core/src/actions.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates
//! why: re-review 3 sweep -- `Unknown` treated as free by an inequality test instead of a match arm (blind spot: the refusal rule only inspects `match` arms whose pattern names `Unknown`; `state != Occupied` never produces one)
/// Sweep: fail-open occupancy, written as an inequality.
pub fn sweep_can_remove(state: crate::occupancy::OccupancyState) -> bool {
    !matches!(state, crate::occupancy::OccupancyState::Occupied(_))
}
