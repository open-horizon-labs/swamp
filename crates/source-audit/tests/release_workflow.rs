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
fn publication_still_requires_both_full_checks_and_all_archive_checks() {
    for name in ["macos-arm64-check-full", "linux-x86_64-check-full"] {
        assert!(job(name).contains("run: scripts/check-full.sh"));
    }
    let publish = job("release");
    let needs = publish.split("runs-on:").next().unwrap();
    for name in [
        "macos-arm64",
        "linux-x86_64",
        "linux-x86_64-newer",
        "macos-arm64-check-full",
        "linux-x86_64-check-full",
    ] {
        assert!(needs.contains(&format!("{name},")), "missing gate {name}");
    }
}
