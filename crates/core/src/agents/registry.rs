//! Static registration for agent-storage adapters, mirroring
//! `crate::locations::Registry` exactly
//! (`.oh/guardrails/agent-adapters-are-pluggable.md`).
//!
//! Before this module, dispatch was a fourteen-arm `match tool_id` in
//! `agents/mod.rs::identify_for_tool`, a second fourteen-arm match in
//! `actions.rs::execute_agent_session_removal`, a hardcoded two-id
//! `multi_location_tool`, and a bespoke call path for Aider. Adding a
//! tool meant editing four places; forgetting one meant a tool that
//! identified fine and then refused to re-verify at execution.
//!
//! Now every adapter is one line here, and `id()` / `capabilities()`
//! answer the questions those matches used to. Nothing registers an
//! adapter at runtime, and the `agent_adapters_are_pluggable` audit
//! checks the registered set against the module set, so an unregistered
//! adapter or a duplicate registration fails the build's own audit
//! rather than silently doing nothing.

use super::AgentAdapter;
use super::{
    aider, claude_code, cline, codex, codex_desktop, continue_dev, copilot_cli, cursor, gemini_cli,
    oh_my_pi, opencode, pi, roo_code, windsurf,
};

pub struct Registry {
    adapters: Vec<Box<dyn AgentAdapter>>,
}

impl Registry {
    pub fn with_builtins() -> Self {
        Self {
            adapters: vec![
                Box::new(claude_code::Adapter),
                Box::new(codex::Adapter),
                Box::new(codex_desktop::Adapter),
                Box::new(oh_my_pi::Adapter),
                Box::new(opencode::Adapter),
                Box::new(gemini_cli::Adapter),
                Box::new(pi::Adapter),
                Box::new(aider::Adapter),
                Box::new(copilot_cli::Adapter),
                Box::new(cursor::Adapter),
                Box::new(windsurf::Adapter),
                Box::new(cline::Adapter),
                Box::new(roo_code::Adapter),
                Box::new(continue_dev::Adapter),
            ],
        }
    }

    pub fn adapters(&self) -> &[Box<dyn AgentAdapter>] {
        &self.adapters
    }

    pub fn get(&self, id: &str) -> Option<&dyn AgentAdapter> {
        self.adapters
            .iter()
            .find(|a| a.id() == id)
            .map(|a| a.as_ref())
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.id()).collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::matrix;

    #[test]
    fn every_adapter_is_registered_exactly_once() {
        let r = Registry::with_builtins();
        let mut ids = r.ids();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "a duplicate registration: {ids:?}");
    }

    #[test]
    fn registry_ids_are_exactly_the_matrix_ids() {
        // The matrix is the published support claim and the registry is
        // what actually runs. A tool in one and not the other is either
        // an undocumented adapter or a documented tool with no code.
        let r = Registry::with_builtins();
        let mut registered: Vec<&str> = r.ids();
        registered.sort_unstable();
        let mut documented: Vec<&str> = matrix::MATRIX.iter().map(|e| e.id.slug()).collect();
        documented.sort_unstable();
        assert_eq!(registered, documented);
    }

    #[test]
    fn every_adapter_id_is_also_a_detector_id() {
        // An adapter's home arrives from the detector of the same id;
        // an adapter whose id no detector proposes can never be reached.
        let detectors = crate::locations::Registry::with_builtins();
        let detector_ids: Vec<&str> = detectors
            .detectors()
            .iter()
            .map(|d| d.id())
            .collect::<Vec<_>>();
        for id in Registry::with_builtins().ids() {
            assert!(
                detector_ids.contains(&id),
                "adapter {id} has no detector to resolve its home"
            );
        }
    }

    #[test]
    fn only_declared_multi_location_adapters_decompose_every_location() {
        // The capability replaced a hardcoded `multi_location_tool`
        // match on two ids; this pins that the replacement did not
        // quietly widen it.
        let r = Registry::with_builtins();
        let every: Vec<&str> = r
            .adapters()
            .iter()
            .filter(|a| a.capabilities().decomposes_every_location)
            .map(|a| a.id())
            .collect();
        assert_eq!(every, vec!["cline", "roo-code"]);
    }

    #[test]
    fn only_aider_declares_project_local_units() {
        let r = Registry::with_builtins();
        let local: Vec<&str> = r
            .adapters()
            .iter()
            .filter(|a| a.capabilities().project_local_units)
            .map(|a| a.id())
            .collect();
        assert_eq!(local, vec!["aider"]);
    }
}
