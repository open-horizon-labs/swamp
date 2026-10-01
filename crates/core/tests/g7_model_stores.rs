//! Model caches end to end (v0.8.0 G7): a Hugging Face hub cache and an
//! Ollama store on a fixture home, through the real pipeline
//! (`report::observe_scope`), then the stored report (`report_scope_from_store`).
//!
//! One test, because it sets `SWAMP_TEST_PROGRAM_DIR` (a fake `curl` that
//! logs each call and never touches the network) for the whole process.
//!
//! What it proves, each against a tempting wrong patch:
//! * downloaded models say how they come back, not "cannot be
//!   regenerated" (wrong patch: leaving the `models` category default);
//! * the Hub is not asked unless `hf_enrich = true` (wrong patch: asking
//!   whenever curl exists);
//! * a second observe over unchanged files reads no file content and asks
//!   the Hub nothing (wrong patch: a cache keyed on time, or no cache);
//! * the TUI's refresh (an observe with `enrich: false`) and `report`
//!   never start curl (wrong patch: gating the fetch on `observe` alone);
//! * the stored report carries the "what it is" line and last-read fact.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

const REV: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

fn scope(home: &Path, env: &HashMap<String, String>) -> EffectiveScope {
    let env = Environment::fixture(home.to_path_buf(), env.clone(), Platform::MacOS);
    resolve_effective_scope(
        &env,
        &ScanConfig {
            defaults: false,
            enabled_detectors: vec!["huggingface".into(), "ollama".into()],
            ..Default::default()
        },
        &[],
        &Registry::with_builtins(),
        1_000,
    )
}

fn observe(s: &EffectiveScope, store: &Path, enrich: bool) -> swamp_core::report::ScopeObservation {
    swamp_core::report::observe_scope(
        s,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        enrich,
        false,
        &swamp_core::fs_events::UnsupportedPlatformSource,
        30,
        3600,
    )
    .expect("observe")
}

fn calls(log: &Path) -> usize {
    // The `-w` argument holds a newline, so count calls, not lines.
    fs::read_to_string(log).map_or(0, |t| t.lines().filter(|l| l.starts_with("-q ")).count())
}

#[test]
fn model_stores_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let home = base.join("home");
    let hub = base.join("hf/hub");
    let ollama = base.join("ollama-models");
    let store = base.join("store");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&store).unwrap();

    // One hub model with a card, config and a safetensors header.
    let repo = hub.join("models--org--tiny");
    fs::create_dir_all(repo.join("blobs")).unwrap();
    fs::create_dir_all(repo.join("refs")).unwrap();
    fs::create_dir_all(repo.join("snapshots").join(REV)).unwrap();
    fs::write(repo.join("refs/main"), REV).unwrap();
    fs::write(
        repo.join("blobs/readme"),
        "---\npipeline_tag: text-generation\nlicense: mit\n---\nA tiny model.\n",
    )
    .unwrap();
    fs::write(repo.join("blobs/config"), r#"{"model_type":"gpt2"}"#).unwrap();
    let json = r#"{"w":{"dtype":"F16","shape":[10,10],"data_offsets":[0,200]}}"#;
    let mut st = (json.len() as u64).to_le_bytes().to_vec();
    st.extend(json.as_bytes());
    st.extend(vec![0u8; 20_000]);
    fs::write(repo.join("blobs/weights"), st).unwrap();
    // Loaded once since download: its access time is after its mtime.
    {
        use std::os::unix::fs::MetadataExt;
        let w = repo.join("blobs/weights");
        let m = fs::metadata(&w).unwrap();
        let at = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs((m.mtime().max(m.ctime()) + 60) as u64);
        fs::File::options()
            .write(true)
            .open(&w)
            .unwrap()
            .set_times(fs::FileTimes::new().set_accessed(at))
            .unwrap();
    }
    for (n, b) in [
        ("README.md", "readme"),
        ("config.json", "config"),
        ("model.safetensors", "weights"),
    ] {
        symlink(
            format!("../../blobs/{b}"),
            repo.join("snapshots").join(REV).join(n),
        )
        .unwrap();
    }
    // One Ollama model.
    let d = |c: char| format!("sha256-{}", c.to_string().repeat(64));
    fs::create_dir_all(ollama.join("blobs")).unwrap();
    fs::create_dir_all(ollama.join("manifests/registry.ollama.ai/library/tiny")).unwrap();
    fs::write(ollama.join("blobs").join(d('a')), vec![3u8; 30_000]).unwrap();
    fs::write(
        ollama.join("blobs").join(d('c')),
        r#"{"model_format":"gguf","model_family":"llama","model_type":"1B","file_type":"Q4_0"}"#,
    )
    .unwrap();
    fs::write(
        ollama.join("manifests/registry.ollama.ai/library/tiny/latest"),
        format!(
            r#"{{"config":{{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:{}","size":80}},"layers":[{{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:{}","size":30000}}]}}"#,
            "c".repeat(64),
            "a".repeat(64)
        ),
    )
    .unwrap();

    // A fake curl: logs, answers 200 with a fixed body. Never the network.
    let programs = base.join("programs");
    fs::create_dir_all(&programs).unwrap();
    let log = base.join("curl.log");
    let curl = programs.join("curl");
    fs::write(
        &curl,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nenv | sort >> '{}.env'\nprintf '%s\\n200' '{{\"sha\":\"{REV}\",\"downloads\":7,\"likes\":2,\"pipeline_tag\":\"text-generation\",\"cardData\":{{\"base_model\":\"org/base\"}}}}'\n",
            log.display(),
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary.
    unsafe {
        std::env::set_var("SWAMP_TEST_PROGRAM_DIR", &programs);
        // What a machine behind a proxy has set; only the https proxy, the
        // no-proxy list and the CA bundle may reach curl.
        std::env::set_var("HTTPS_PROXY", "http://proxy.test:3128");
        std::env::set_var("no_proxy", "localhost");
        std::env::set_var("SSL_CERT_FILE", "/etc/ssl/cert.pem");
        std::env::set_var("HTTP_PROXY", "http://plain.test:80");
        std::env::set_var("HF_TOKEN", "hf_secret_never_sent");
        std::env::set_var("CURL_HOME", "/tmp/evil");
    }

    let mut env = HashMap::new();
    env.insert("HF_HOME".to_string(), base.join("hf").display().to_string());
    env.insert("OLLAMA_MODELS".to_string(), ollama.display().to_string());
    let s = scope(&home, &env);

    // 1. Off by default: a full observe asks the Hub nothing.
    let first = observe(&s, &store, true);
    assert_eq!(calls(&log), 0, "the Hub was asked with hf_enrich off");
    let tiny = first
        .store_interiors
        .iter()
        .find(|u| u.path == repo)
        .unwrap_or_else(|| panic!("no hub repo unit: {:#?}", first.store_interiors));
    let rows = swamp_core::build_adapters::model_stores::model_rows(
        &hub,
        &first.store_interiors,
        2_000_000_000,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].about.as_deref(),
        Some("text-generation · gpt2 · 100 params · f16 · license mit"),
        "{rows:#?}"
    );
    assert!(
        rows[0]
            .regeneration
            .starts_with("downloaded again from huggingface.co (org/tiny@abcdefab) when needed"),
        "{rows:#?}"
    );
    assert_eq!(rows[0].hub.as_deref(), Some("off"));
    assert!(tiny.coverage.supported);
    let text = swamp_core::render::model_lines(&rows).join("\n");
    assert!(text.contains("`hf_enrich = true`"), "{text}");
    let orows = swamp_core::build_adapters::model_stores::model_rows(
        &ollama,
        &first.store_interiors,
        2_000_000_000,
    );
    assert_eq!(orows[0].about.as_deref(), Some("llama · 1B params · Q4_0"));
    assert!(orows[0].regeneration.contains("`ollama pull tiny:latest`"));
    // The hub unit's drilldown row for the repo has the adapter's last read.
    let hub_unit = first
        .external_units
        .iter()
        .find(|u| u.path == hub)
        .expect("hub external unit");
    let child = hub_unit
        .children
        .iter()
        .find(|c| c.name == "models--org--tiny")
        .unwrap_or_else(|| panic!("no repo row: {:#?}", hub_unit.children));
    assert!(child.last_used.at.is_some(), "{child:?}");

    // 2. Warm: nothing changed, no content is read again.
    let (_, warm) = swamp_core::work_counters::measured(|| observe(&s, &store, true));
    assert_eq!(
        warm.header_bytes_read, 0,
        "warm observe read file content: {warm:?}"
    );

    // 3. Turned on: one request per hub repo, then none while fresh.
    fs::write(store.join("config.toml"), "hf_enrich = true\n").unwrap();
    let on = observe(&s, &store, true);
    assert_eq!(calls(&log), 1);
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("https://huggingface.co/api/models/org/tiny") && logged.starts_with("-q "),
        "{logged}"
    );
    let rows = swamp_core::build_adapters::model_stores::model_rows(
        &hub,
        &on.store_interiors,
        2_000_000_000,
    );
    assert!(
        rows[0]
            .hub
            .as_deref()
            .unwrap()
            .starts_with("from huggingface.co, fetched "),
        "{rows:#?}"
    );
    assert!(rows[0].about.as_deref().unwrap().contains("base org/base"));
    let env = fs::read_to_string(format!("{}.env", log.display())).unwrap();
    let names: Vec<&str> = env
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, _)| k))
        .collect();
    for want in ["HTTPS_PROXY", "no_proxy", "SSL_CERT_FILE"] {
        assert!(
            names.contains(&want),
            "{want} did not reach curl: {names:?}"
        );
    }
    for never in ["HTTP_PROXY", "HF_TOKEN", "CURL_HOME"] {
        assert!(!names.contains(&never), "{never} reached curl: {names:?}");
    }
    assert!(!env.contains("hf_secret") && !logged.contains("hf_secret"));
    let _ = observe(&s, &store, true);
    assert_eq!(calls(&log), 1, "a fresh cached answer was fetched again");

    // 4. The TUI's refresh (enrich: false) and `report` never start curl.
    fs::remove_file(store.join("associations/model_cards.parquet")).unwrap();
    let (_, refresh) = swamp_core::work_counters::measured(|| observe(&s, &store, false));
    assert_eq!(calls(&log), 1, "the TUI refresh asked the Hub");
    let _ = refresh;
    let (snap, read) = swamp_core::work_counters::measured(|| {
        swamp_core::report::report_scope_from_store(&s, &store)
    });
    assert_eq!(read.subprocess_spawns, 0);
    assert_eq!(calls(&log), 1);
    let snap = snap.expect("stored report");
    assert!(
        snap.store_interiors.iter().any(|u| u.path == repo
            && u.producer_evidence
                .iter()
                .any(|e| e.source == swamp_core::build_adapters::model_stores::CARD_EVIDENCE)),
        "the stored report lost the card"
    );
    let _: PathBuf = home;
}
