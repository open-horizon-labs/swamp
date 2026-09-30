//! v0.8.0 G4a verification (audit/v080-g4ab): parser edges on the fix
//! round head ed1f125.

use swamp_core::locations::SubjectShape;
use swamp_core::manager_facts::{parse_toolchain, subject_matches};

/// Tempting wrong patch: slice `r[8..10]` before checking that byte 10 is
/// a boundary. A toolchain folder (or a settings.toml default) whose
/// eleventh byte after the channel is inside a multi-byte character makes
/// the byte slice panic, and the whole Reclaim view with it.
#[test]
fn ver_parse_toolchain_never_panics_on_a_multibyte_name() {
    for name in [
        "nightly-2024-01-0\u{e9}x",
        "stable-2024-01-01\u{e9}",
        "1.90-2024-01-0\u{1F600}",
        "beta-\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
    ] {
        let r = std::panic::catch_unwind(|| parse_toolchain(name).map(|t| t.channel.len()));
        assert!(r.is_ok(), "parse_toolchain panicked on {name:?}");
    }
}

/// mise names a backend tool's install folder by kebab-casing the whole
/// backend argument. Verified on mise 2026.9.15 with fake install folders
/// under a sandboxed MISE_DATA_DIR: `npm:@scope/pkg` -> `npm-scope-pkg`,
/// `ubi:BurntSushi/ripgrep` -> `ubi-burnt-sushi-ripgrep`. Tempting wrong
/// patch: replace `:` and `/` with `-` and nothing else.
#[test]
fn ver_mise_backend_folder_names_follow_mises_own_rule() {
    for (subject, folder) in [
        ("npm:prettier", "npm-prettier"),
        ("aqua:owner/repo", "aqua-owner-repo"),
        ("npm:@scope/pkg", "npm-scope-pkg"),
        ("npm:@scope/pkg@1.0.0", "npm-scope-pkg"),
        ("ubi:BurntSushi/ripgrep", "ubi-burnt-sushi-ripgrep"),
    ] {
        assert!(
            subject_matches(SubjectShape::NameBeforeAt, subject, folder),
            "{subject} should name the folder {folder}"
        );
    }
}

/// Dated channels and host triples that the fix round claims to handle.
#[test]
fn ver_toolchain_matching_table() {
    let yes = [
        (
            "nightly-2024-01-01",
            "nightly-2024-01-01-aarch64-apple-darwin",
        ),
        ("stable", "stable-aarch64-apple-darwin"),
        ("1.90", "1.90-x86_64-unknown-linux-gnu"),
        ("1.90.0", "1.90.0-x86_64-pc-windows-msvc"),
        ("stable-aarch64-apple-darwin", "stable-aarch64-apple-darwin"),
        ("my-linked", "my-linked"),
    ];
    for (s, f) in yes {
        assert!(
            subject_matches(SubjectShape::ChannelWithHostTriple, s, f),
            "{s} vs {f}"
        );
    }
    let no = [
        ("nightly", "nightly-2024-01-01-aarch64-apple-darwin"),
        ("stable-x86_64-apple-darwin", "stable-aarch64-apple-darwin"),
        ("1.90", "1.90.0-aarch64-apple-darwin"),
    ];
    for (s, f) in no {
        assert!(
            !subject_matches(SubjectShape::ChannelWithHostTriple, s, f),
            "{s} vs {f}"
        );
    }
}
