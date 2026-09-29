//! Regression checks for the release's division of work. These deliberately
//! check the current workflow layout, not arbitrary GitHub Actions semantics.

const WORKFLOW: &str = include_str!("../../../.github/workflows/release.yml");

fn job(name: &str) -> &str {
    let heading = format!("\n  {name}:\n");
    let start = WORKFLOW.find(&heading).expect("release job exists") + heading.len();
    let body = &WORKFLOW[start..];
    let end = body
        .match_indices("\n  ")
        .find(|(index, _)| {
            body.as_bytes()
                .get(index + 3)
                .is_some_and(|c| !c.is_ascii_whitespace())
        })
        .map_or(body.len(), |(index, _)| index);
    &body[..end]
}

#[test]
fn packaging_tests_archives_without_rebuilding_workspace_tests() {
    for name in ["macos-arm64", "linux-x86_64"] {
        let body = job(name);
        assert!(body.contains("cargo build --release --locked"), "{name}");
        assert!(body.contains("./scripts/package-release.sh"), "{name}");
        assert!(body.contains("./scripts/release-smoke.sh"), "{name}");
        assert!(
            !body.contains("cargo test"),
            "{name}: full suite belongs in check-full"
        );
    }
    let newer = job("linux-x86_64-newer");
    assert!(newer.contains("needs: linux-x86_64"));
    assert!(newer.contains("actions/download-artifact@"));
    assert!(newer.contains("./scripts/release-smoke.sh"));
    assert!(!newer.contains("cargo "));
    assert!(!newer.contains("rust-toolchain"));
}

#[test]
fn publication_still_requires_ci_the_full_tier_and_all_archive_checks() {
    // The release waits for `ci.yml` and the full tier to have passed on
    // this exact commit; it does not rebuild and re-run either (the full
    // tier alone is about 27 minutes).
    let gate = job("gates");
    assert!(gate.contains("scripts/verify-gates.sh"));
    assert!(
        !gate.contains("scripts/check-full.sh"),
        "the gate must not re-run the tier"
    );
    // The script's own default is the release's contract, so it must name
    // both workflows.
    let script = include_str!("../../../scripts/verify-gates.sh");
    assert!(
        script.contains("VERIFY_WORKFLOWS:-ci.yml check-full.yml"),
        "the gate must require both ci.yml and check-full.yml"
    );
    let publish = job("release");
    let needs = publish.split("runs-on:").next().unwrap();
    for name in ["macos-arm64", "linux-x86_64", "linux-x86_64-newer", "gates"] {
        assert!(needs.contains(&format!("{name},")), "missing gate {name}");
    }
    for name in ["macos-arm64-check-full", "linux-x86_64-check-full"] {
        assert!(
            !WORKFLOW.contains(&format!("\n  {name}:\n")),
            "{name}: the full tier lives in check-full.yml"
        );
    }
}

const CHECK_FULL: &str = include_str!("../../../.github/workflows/check-full.yml");

#[test]
fn the_full_tier_runs_on_main_so_a_tag_finds_it_already_done() {
    let on = CHECK_FULL.split("\npermissions:").next().unwrap();
    assert!(
        on.contains("push:\n    branches: [main]"),
        "must run on every push to main"
    );
    // Per commit, so a later push to main cannot cancel the run a tag waits on.
    assert!(CHECK_FULL.contains("github.event.pull_request.number || github.sha"));
    // The sweep is Linux's alone; macOS opts out of it.
    let macos = CHECK_FULL
        .split("\n  macos-arm64-check-full:")
        .nth(1)
        .unwrap();
    assert!(macos.contains("SWAMP_SKIP_MUTATION_SWEEP"));
    let linux = CHECK_FULL
        .split("\n  linux-x86_64-check-full:")
        .nth(1)
        .unwrap()
        .split("\n  macos-arm64-check-full:")
        .next()
        .unwrap();
    // Linux skips the sweep only when it already passed for this clippy
    // version and these inputs: the key must carry both, the marker must be
    // saved only after the tier passed, and there is no schedule.
    assert!(
        linux.contains("cargo clippy --version"),
        "the key must include the clippy version"
    );
    assert!(
        linux.contains("crates/source-audit/**"),
        "the key must hash the audit crate"
    );
    assert!(
        linux.contains("Cargo.lock"),
        "the key must hash the lockfile"
    );
    assert!(linux.contains("scripts/sweep-needed.sh"));
    assert!(
        linux.contains("SWAMP_SKIP_MUTATION_SWEEP: ${{ steps.sweep.outputs.verdict == 'skip'"),
        "Linux may skip the sweep only on the script's verdict, never unconditionally"
    );
    let check = linux
        .find("scripts/check-full.sh\n")
        .expect("check-full step");
    let save = linux.find("actions/cache/save@").expect("marker save step");
    assert!(
        save > check,
        "the marker must be saved only after the tier ran"
    );
    assert!(
        !CHECK_FULL.contains("schedule:"),
        "the sweep has no schedule"
    );
}
