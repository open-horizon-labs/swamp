use std::fs;
use std::process::Command;

#[test]
fn inspect_cargo_is_explicit_and_emits_json_for_selected_profile() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("debug");
    fs::create_dir_all(profile.join("deps")).unwrap();
    fs::create_dir_all(profile.join(".fingerprint")).unwrap();
    fs::write(profile.join("deps/libunknown-abc.rlib"), b"artifact").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_swamp"))
        .args(["inspect-cargo"])
        .arg(&profile)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["profile_path"], profile.to_string_lossy().as_ref());
    assert_eq!(
        value["groups"][0]["residual_reason"],
        "no unique target-named fingerprint matched this artifact"
    );
    assert!(
        value["accounting_note"]
            .as_str()
            .unwrap()
            .contains("not reclaimable")
    );
}
