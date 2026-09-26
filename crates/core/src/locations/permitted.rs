//! Which detectors one invocation may run: the only input
//! [`super::Registry::resolve`] accepts besides the environment.
//!
//! `defaults = false` means explicit-only scope
//! (`.oh/guardrails/explicit-only-scope-when-defaults-false.md`): the
//! builtin-defaults detector never runs, and -- unless the config names
//! detectors -- neither does any other one. That decision is made here,
//! from the whole [`crate::scope::ScanConfig`], and nowhere else: the
//! field is private and the only constructor reads the config, so a
//! resolver that is handed the config's parts (`defaults: bool`, an
//! empty disable list) and runs every detector does not compile.
//!
//! Under ordinary `defaults = true` scope, a detector can also opt out
//! on its own (`Detector::default_enabled` -- a system-wide install
//! tree like Homebrew, off unless `enabled_detectors` names it): that
//! decision is made here too, so `swamp scope` can tell "you disabled
//! this" apart from "this is off unless you turn it on" via
//! [`PermittedDetectors::default_off`].

use super::Registry;
use crate::scope::ScanConfig;

/// The detector ids this invocation must not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermittedDetectors {
    disabled: Vec<String>,
    /// The subset of `disabled` that is off because the detector itself
    /// defaults to off, not because the user's config named it.
    default_off: Vec<String>,
}

impl PermittedDetectors {
    pub fn from_config(config: &ScanConfig, registry: &Registry) -> PermittedDetectors {
        let permitted = crate::scope::detectors_permitted(config);
        let mut disabled = config.disabled_detectors.clone();
        let mut default_off: Vec<String> = Vec::new();
        if !config.defaults {
            disabled.push(super::builtin::BUILTIN_DEFAULTS_DETECTOR_ID.to_string());
            for d in registry.detectors() {
                let id = d.id().to_string();
                if !permitted
                    || (!config.enabled_detectors.is_empty()
                        && !config.enabled_detectors.contains(&id))
                {
                    disabled.push(id);
                }
            }
        } else {
            // A detector that opts out of ordinary `defaults = true`
            // scope (`Detector::default_enabled` returning `false`, a
            // system-wide install tree like Homebrew) stays off unless
            // `enabled_detectors` names it explicitly -- the same
            // config key `defaults = false` already reads as "turned on"
            // for the opposite direction, so one key means "on" in both
            // scan modes rather than needing a second one.
            for d in registry.detectors() {
                let id = d.id().to_string();
                if !d.default_enabled() && !config.enabled_detectors.contains(&id) {
                    disabled.push(id.clone());
                    default_off.push(id);
                }
            }
        }
        disabled.sort();
        disabled.dedup();
        default_off.sort();
        default_off.dedup();
        PermittedDetectors {
            disabled,
            default_off,
        }
    }

    /// The disabled ids, sorted, as the effective scope records them.
    /// Includes both a user's `disabled_detectors` and any detector that
    /// defaults to off and was not explicitly enabled -- see
    /// [`Self::default_off`] to tell the two apart for display.
    pub fn disabled(&self) -> Vec<String> {
        self.disabled.clone()
    }

    /// The subset of [`Self::disabled`] that is off because
    /// `Detector::default_enabled` said so, not because the user's
    /// config named it -- `swamp scope` reports these as `disabled
    /// (default off)` rather than plain `disabled`.
    pub fn default_off(&self) -> Vec<String> {
        self.default_off.clone()
    }

    /// Whether detector `id` may run.
    pub fn permits(&self, id: &str) -> bool {
        !self.disabled.iter().any(|d| d == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homebrew_is_off_under_ordinary_defaults_true_scope() {
        let registry = Registry::with_builtins();
        let config = ScanConfig::default();
        let permitted = PermittedDetectors::from_config(&config, &registry);
        assert!(!permitted.permits("homebrew"));
        assert!(permitted.default_off().contains(&"homebrew".to_string()));
        // An ordinary opt-out detector is unaffected.
        assert!(permitted.permits("cargo-home"));
        assert!(!permitted.default_off().contains(&"cargo-home".to_string()));
    }

    #[test]
    fn enabled_detectors_turns_homebrew_on_under_defaults_true() {
        let registry = Registry::with_builtins();
        let config = ScanConfig {
            enabled_detectors: vec!["homebrew".to_string()],
            ..ScanConfig::default()
        };
        let permitted = PermittedDetectors::from_config(&config, &registry);
        assert!(permitted.permits("homebrew"));
        assert!(!permitted.default_off().contains(&"homebrew".to_string()));
    }

    #[test]
    fn disabled_detectors_still_turns_homebrew_off_explicitly_under_defaults_true() {
        // A detector already off by default, also named in
        // `disabled_detectors`, is still disabled -- and still reported
        // as default-off, since the config's deny-list did not need to
        // do anything for it to be off.
        let registry = Registry::with_builtins();
        let config = ScanConfig {
            disabled_detectors: vec!["homebrew".to_string()],
            ..ScanConfig::default()
        };
        let permitted = PermittedDetectors::from_config(&config, &registry);
        assert!(!permitted.permits("homebrew"));
        assert!(permitted.default_off().contains(&"homebrew".to_string()));
    }

    #[test]
    fn defaults_false_never_reports_default_off_since_everything_is_explicit_there() {
        let registry = Registry::with_builtins();
        let config = ScanConfig {
            defaults: false,
            enabled_detectors: vec!["cargo-home".to_string()],
            ..ScanConfig::default()
        };
        let permitted = PermittedDetectors::from_config(&config, &registry);
        assert!(
            permitted.default_off().is_empty(),
            "explicit-only scope has its own reason for every absence; \
             `default_off` is specifically the ordinary-scope opt-in gap"
        );
    }
}
