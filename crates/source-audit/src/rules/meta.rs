//! Guardrail, ADR and test-contract metadata, checked as data
//! (`guardrail_metadata`).
//!
//! * A hard guardrail names a registered audit, or says `audit: none`
//!   with a dated `audit_none_reason:` and at least one `compile_fail:`
//!   case or `runtime_tests:` entry that watches it instead. Every
//!   `compile_fail:` case exists (`crates/core/tests/compile_fail/<case>.rs`
//!   with its expected `.stderr`); every runtime test is a `#[test]` in a
//!   file Cargo actually builds (a top-level `crates/*/tests/*.rs` target
//!   or a module one declares, or the crate's own module tree), is not
//!   ignored in any spelling (`#[ignore]`, `#[cfg_attr(.., ignore)]`),
//!   and invokes an assertion macro (a *macro invocation*, not the
//!   substring `assert` in a local's name).
//! * Every guardrail's `## Detection` section says how it is detected on
//!   a `Mechanism:` line: `type`, `gate audit`, `clippy` and/or `runtime
//!   test`.
//! * An ADR's `validate.audits` resolve and its `cargo_tests` are
//!   running, asserting tests.
//! * Every agent adapter module proves the same five things about itself
//!   as running, asserting tests in its own file
//!   (`.oh/guardrails/agent-adapter-test-contract.md`).

use super::{Rule, verdict};
use crate::model::{TestFn, Workspace, compiled_tests};
use std::path::Path;

fn frontmatter(text: &str) -> Vec<String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Vec::new();
    }
    lines
        .take_while(|l| l.trim() != "---")
        .map(str::to_string)
        .collect()
}

fn value(front: &[String], key: &str) -> Option<String> {
    front
        .iter()
        .find_map(|l| {
            l.strip_prefix(&format!("{key}:"))
                .map(|v| v.trim().trim_matches('"').to_string())
        })
        .filter(|v| !v.is_empty())
}

fn list(front: &[String], key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut on = false;
    for l in front {
        let t = l.trim();
        if t == format!("{key}:") {
            on = true;
            continue;
        }
        if on {
            match t.strip_prefix("- ") {
                Some(item) => out.push(item.trim().trim_matches('"').to_string()),
                None => on = false,
            }
        }
    }
    out
}

fn section(text: &str, name: &str) -> Option<String> {
    let mut out = String::new();
    let mut on = false;
    let mut found = false;
    for l in text.lines() {
        if l.starts_with("## ") {
            on = l
                .trim_start_matches('#')
                .trim()
                .to_ascii_lowercase()
                .starts_with(name);
            found |= on;
            continue;
        }
        if on {
            out.push_str(l);
            out.push('\n');
        }
    }
    found.then_some(out)
}

fn dated(s: &str) -> bool {
    s.as_bytes().windows(10).any(|w| {
        w.iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    })
}

const ASSERTING_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "assert_matches",
    "debug_assert",
    "panic",
];

/// Whether a test asserts something: an assertion macro invocation, or a
/// call into a shared `contract` helper (which asserts).
pub fn asserts(t: &TestFn) -> bool {
    t.macros
        .iter()
        .any(|m| ASSERTING_MACROS.contains(&m.as_str()))
        || t.calls
            .iter()
            .any(|c| c.split("::").any(|s| s == "contract"))
}

/// The running, asserting test a spec names: `file.rs::name`, a bare
/// `name`, or a whole test file (`crates/x/tests/y.rs`), which needs at
/// least one running, asserting test of its own.
fn running<'a>(tests: &'a [TestFn], spec: &str) -> Option<&'a TestFn> {
    if spec.ends_with(".rs") {
        return tests
            .iter()
            .find(|t| t.file == spec && !t.ignored && asserts(t));
    }
    let (file, name) = match spec.split_once(".rs::") {
        Some((f, n)) => (Some(format!("{f}.rs")), n.rsplit("::").next().unwrap_or(n)),
        None => (None, spec.rsplit("::").next().unwrap_or(spec)),
    };
    tests.iter().find(|t| {
        t.name == name && !t.ignored && asserts(t) && file.as_ref().is_none_or(|f| &t.file == f)
    })
}

const MECHANISMS: &[&str] = &["type", "gate audit", "clippy", "runtime test"];

/// The five facts every agent adapter proves about itself.
pub const REQUIRED_ADAPTER_TESTS: &[&str] = &[
    "unknown_format_is_explicit_not_empty",
    "canary_content_never_appears_in_output",
    "identification_reads_no_more_than_header_cap",
    "protected_categories_default_protected",
    "project_link_is_declared_or_unresolved_never_basename_guess",
];

fn adapter_test_contract(module_file: &str) -> Vec<&'static str> {
    REQUIRED_ADAPTER_TESTS
        .iter()
        .map(|name| {
            if *name == "identification_reads_no_more_than_header_cap"
                && module_file == "crates/core/src/agents/codex.rs"
            {
                "identification_reads_no_rollout_header_bytes"
            } else {
                name
            }
        })
        .collect()
}

pub fn guardrail_metadata(ws: &Workspace) -> Vec<String> {
    let root = &ws.root;
    let names = crate::audits::names();
    let (tests, test_errors) = compiled_tests(root);
    let mut problems: Vec<String> = test_errors
        .into_iter()
        .map(|e| format!("a test file does not parse: {e}"))
        .collect();
    let gdir = root.join(".oh/guardrails");
    let mut entries: Vec<_> = match std::fs::read_dir(&gdir) {
        Ok(rd) => rd.flatten().collect(),
        Err(e) => return vec![format!("read .oh/guardrails: {e}")],
    };
    entries.sort_by_key(|e| e.path());
    let mut seen = 0;
    for e in entries {
        if e.path().extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let rel = format!(".oh/guardrails/{}", e.file_name().to_string_lossy());
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            problems.push(format!("{rel}: unreadable"));
            continue;
        };
        seen += 1;
        let front = frontmatter(&text);
        let severity = value(&front, "severity").unwrap_or_default();
        let compile_fail = list(&front, "compile_fail");
        let runtime = list(&front, "runtime_tests");
        for case in &compile_fail {
            let rs = root.join(format!("crates/core/tests/compile_fail/{case}.rs"));
            let stderr = root.join(format!("crates/core/tests/compile_fail/{case}.stderr"));
            if !rs.is_file() || !stderr.is_file() {
                problems.push(format!(
                    "{rel}: compile_fail case `{case}` has no crates/core/tests/compile_fail/{case}.rs \
                     and .stderr pair"
                ));
            }
        }
        for t in &runtime {
            if running(&tests, t).is_none() {
                problems.push(format!(
                    "{rel}: runtime test `{t}` is not a #[test] Cargo builds, runs (not ignored in \
                     any spelling) and that invokes an assertion"
                ));
            }
        }
        match value(&front, "audit").as_deref() {
            None if severity == "hard" => problems.push(format!(
                "{rel}: `severity: hard` with no `audit:` -- name the registered audit that \
                 watches it, or say `audit: none` with a dated reason and the compile_fail cases \
                 or runtime tests that watch it instead"
            )),
            None => {}
            Some("none") => {
                let reason = value(&front, "audit_none_reason").unwrap_or_default();
                if severity == "hard"
                    && (!dated(&reason) || (runtime.is_empty() && compile_fail.is_empty()))
                {
                    problems.push(format!(
                        "{rel}: `audit: none` on a hard guardrail needs a dated \
                         `audit_none_reason:` and a non-empty `compile_fail:` or `runtime_tests:` \
                         list"
                    ));
                }
            }
            Some(a) => {
                for one in a.split(',').map(str::trim) {
                    if !names.contains(&one) {
                        problems.push(format!(
                            "{rel}: names audit `{one}`, which is not registered"
                        ));
                    }
                }
            }
        }
        match section(&text, "detection") {
            None => problems.push(format!("{rel}: no `## Detection` section")),
            Some(det) => {
                let line = det
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("Mechanism:"))
                    .map(str::to_ascii_lowercase);
                match line {
                    Some(l) if MECHANISMS.iter().any(|m| l.contains(m)) => {}
                    _ => problems.push(format!(
                        "{rel}: its Detection section has no `Mechanism:` line naming one of \
                         {MECHANISMS:?}"
                    )),
                }
            }
        }
    }
    if seen == 0 {
        problems.push(".oh/guardrails is empty".into());
    }
    // ADRs.
    if let Ok(rd) = std::fs::read_dir(root.join("docs/ADRs")) {
        let mut adrs: Vec<_> = rd.flatten().collect();
        adrs.sort_by_key(|e| e.path());
        for e in adrs {
            if e.path().extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let rel = format!("docs/ADRs/{}", e.file_name().to_string_lossy());
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            let front = frontmatter(&text);
            for a in list(&front, "audits") {
                if !names.contains(&a.as_str()) {
                    problems.push(format!("{rel}: names audit `{a}`, which is not registered"));
                }
            }
            for t in list(&front, "cargo_tests") {
                if running(&tests, &t).is_none() {
                    problems.push(format!(
                        "{rel}: names test `{t}`, which is not a running, asserting #[test]"
                    ));
                }
            }
        }
    }
    // Adapter test contracts.
    for m in &ws.modules {
        if !crate::rules::gate::is_adapter(m) || m.test {
            continue;
        }
        if !m.str_consts.iter().any(|(n, _, _)| n.ends_with("_TOOL_ID")) {
            continue;
        }
        let missing: Vec<&str> = adapter_test_contract(&m.file)
            .into_iter()
            .filter(|name| {
                !tests
                    .iter()
                    .any(|t| t.file == m.file && t.name == *name && !t.ignored && asserts(t))
            })
            .collect();
        if !missing.is_empty() {
            problems.push(format!(
                "{}: adapter {} is missing running, asserting contract tests: {}",
                m.file,
                m.display(),
                missing.join(", ")
            ));
        }
    }
    problems
}

pub fn guardrail_metadata_at(root: &Path) -> Result<(), String> {
    let ws = Workspace::load(root);
    verdict(
        "guardrail, ADR and contract-test metadata resolve to real audits, compile-fail cases \
         and running tests",
        guardrail_metadata(&ws),
    )
}

pub const RULES: &[Rule] = &[("guardrail_metadata", |ws| {
    verdict(
        "guardrail, ADR and contract-test metadata resolve to real audits, compile-fail cases and \
         running tests",
        guardrail_metadata(ws),
    )
})];

#[cfg(test)]
mod tests {
    use super::adapter_test_contract;

    #[test]
    fn codex_requires_the_stronger_zero_rollout_bytes_contract() {
        let codex = adapter_test_contract("crates/core/src/agents/codex.rs");
        assert!(codex.contains(&"identification_reads_no_rollout_header_bytes"));
        assert!(!codex.contains(&"identification_reads_no_more_than_header_cap"));

        let other = adapter_test_contract("crates/core/src/agents/claude_code.rs");
        assert!(other.contains(&"identification_reads_no_more_than_header_cap"));
        assert!(!other.contains(&"identification_reads_no_rollout_header_bytes"));
    }
}
