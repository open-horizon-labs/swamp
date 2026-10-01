use super::*;
use crate::build_adapters::{ContainerCache, FoldedDir, FoldedIndex};
use crate::fs_events::EventCoverage;
use std::fs;
use std::os::unix::fs::symlink;

const REV1: &str = "1111111111111111111111111111111111111111";
const REV2: &str = "2222222222222222222222222222222222222222";
const REV3: &str = "3333333333333333333333333333333333333333";

fn st_header(tensors: &[(&str, &str, &[u64])]) -> Vec<u8> {
    let body: Vec<String> = tensors
        .iter()
        .map(|(n, d, s)| {
            format!(
                r#""{n}":{{"dtype":"{d}","shape":{:?},"data_offsets":[0,0]}}"#,
                s
            )
        })
        .collect();
    let json = format!("{{{}}}", body.join(","));
    let mut b = (json.len() as u64).to_le_bytes().to_vec();
    b.extend(json.as_bytes());
    b.extend(vec![0u8; 4096]);
    b
}

fn allocated(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    crate::fs_gate::symlink_metadata(p).map_or(0, |m| m.blocks() * 512)
}

struct Hub {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// A hub cache with: a repo whose two revisions share one weight blob, a
/// dangling snapshot link, a link escaping the cache and an incomplete
/// download (`org/a`); two repos linking the same blob in the hub's
/// shared `blobs/` (`org/b`, `org/c`); a hub blob nothing links; a
/// locally made repo with no ref (`me/tuned`).
fn hub_fixture(readme: &str) -> Hub {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let a = root.join("models--org--a");
    for d in ["blobs", "refs"] {
        fs::create_dir_all(a.join(d)).unwrap();
    }
    fs::write(a.join("blobs/h-readme"), readme).unwrap();
    fs::write(
        a.join("blobs/h-config"),
        r#"{"model_type":"llama","architectures":["LlamaForCausalLM"],"torch_dtype":"bfloat16"}"#,
    )
    .unwrap();
    fs::write(
        a.join("blobs/h-weights"),
        st_header(&[("w", "BF16", &[1000, 1000]), ("b", "BF16", &[1000])]),
    )
    .unwrap();
    fs::write(a.join("blobs/h-tok"), "{}").unwrap();
    fs::write(a.join("blobs/h-part.incomplete"), vec![0u8; 9000]).unwrap();
    fs::write(a.join("refs/main"), REV1).unwrap();
    for (rev, files) in [
        (
            REV1,
            vec![
                ("README.md", "../../blobs/h-readme"),
                ("config.json", "../../blobs/h-config"),
                ("model.safetensors", "../../blobs/h-weights"),
                ("tokenizer.json", "../../blobs/h-tok"),
                ("gone.bin", "../../blobs/h-missing"),
                ("escape.txt", "../../../../../../etc/hosts"),
            ],
        ),
        (REV2, vec![("model.safetensors", "../../blobs/h-weights")]),
    ] {
        let snap = a.join("snapshots").join(rev);
        fs::create_dir_all(&snap).unwrap();
        for (n, t) in files {
            symlink(t, snap.join(n)).unwrap();
        }
    }
    // The hub's shared blob folder, and two repos linking one blob in it.
    fs::create_dir_all(root.join("blobs/ab")).unwrap();
    fs::create_dir_all(root.join("blobs/cd")).unwrap();
    fs::write(
        root.join("blobs/ab/abhash"),
        st_header(&[("x", "F32", &[64, 64])]),
    )
    .unwrap();
    fs::write(root.join("blobs/cd/loose"), vec![1u8; 5000]).unwrap();
    for (name, rev) in [("models--org--b", REV3), ("models--org--c", REV3)] {
        let r = root.join(name);
        fs::create_dir_all(r.join("blobs")).unwrap();
        fs::create_dir_all(r.join("refs")).unwrap();
        fs::create_dir_all(r.join("snapshots").join(rev)).unwrap();
        symlink("../../blobs/ab/abhash", r.join("blobs/hb")).unwrap();
        fs::write(r.join("refs/main"), rev).unwrap();
        symlink(
            "../../blobs/hb",
            r.join("snapshots").join(rev).join("model.safetensors"),
        )
        .unwrap();
    }
    let t = root.join("models--me--tuned");
    fs::create_dir_all(t.join("blobs")).unwrap();
    fs::create_dir_all(t.join("snapshots/my-run")).unwrap();
    fs::write(t.join("blobs/w"), vec![2u8; 3000]).unwrap();
    symlink("../../blobs/w", t.join("snapshots/my-run/model.bin")).unwrap();
    Hub { _tmp: tmp, root }
}

/// Folded rows as the walk would give them: each directory's own
/// allocated bytes (symlinks counted as the walk counts them: not at
/// all), summed upward.
fn folded(root: &Path) -> FoldedIndex {
    fn total(p: &Path, out: &mut Vec<FoldedDir>) -> u64 {
        let mut sum = 0;
        let (entries, _) = crate::locations::shallow_list_links(p);
        for e in entries {
            let c = p.join(&e.name);
            if e.is_symlink {
                continue;
            }
            if e.is_dir {
                sum += total(&c, out);
            } else {
                sum += allocated(&c);
            }
        }
        out.push(FoldedDir {
            path: p.to_path_buf(),
            allocated_total: sum,
            mtime_max: 5,
            complete: true,
        });
        sum
    }
    let mut out = Vec::new();
    total(root, &mut out);
    FoldedIndex::from_dirs(out)
}

fn identify(
    kind: BuildStoreKind,
    root: &Path,
    cards: &CardCache,
) -> (Vec<NestedArtifact>, crate::work_counters::WorkCounters) {
    let idx = folded(root);
    let none = EventCoverage::untrusted();
    let cache = ContainerCache::disabled();
    let c = BuildContainer::shared_store_of("model-stores", root.to_path_buf(), kind);
    let ctx = BuildCtx::new(1_000_000, &idx, &none, &cache).with_cards(cards);
    crate::work_counters::measured(|| Adapter.identify(&c, &ctx))
}

fn unit<'a>(units: &'a [NestedArtifact], leaf: &str) -> &'a NestedArtifact {
    units
        .iter()
        .find(|u| u.path.file_name().is_some_and(|n| n == leaf))
        .unwrap_or_else(|| panic!("no unit {leaf}: {units:#?}"))
}

fn ev<'a>(u: &'a NestedArtifact, source: &str) -> Vec<&'a str> {
    u.producer_evidence
        .iter()
        .filter(|e| e.source == source)
        .map(|e| e.detail.as_str())
        .collect()
}

fn text_of(u: &NestedArtifact) -> String {
    format!("{u:?}")
}

const README: &str = "---\nlicense: llama3\npipeline_tag: text-generation\nbase_model: meta-llama/Llama-3.1-8B\n---\n# A\n\nA small \u{202E}test model\u{1b}[2J for fixtures.\n";

/// Tempting wrong patch: summing every snapshot file's size, or
/// following the links, which counts `h-weights` once per revision and
/// the hub's shared blob once per repo. Units must sum to the store.
#[test]
fn blobs_are_counted_once_and_units_sum_to_the_store() {
    let h = hub_fixture(README);
    let cards = CardCache::default();
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &h.root, &cards);
    let idx = folded(&h.root);
    let store_total = idx.get(&h.root).unwrap().allocated_total;
    let sum: u64 = units
        .iter()
        .filter(|u| u.path != h.root)
        .map(|u| u.bytes)
        .sum();
    assert_eq!(sum, store_total, "{units:#?}");
    let a = unit(&units, "models--org--a");
    assert_eq!(
        a.bytes,
        idx.get(&h.root.join("models--org--a"))
            .unwrap()
            .allocated_total
    );
    let shared = allocated(&h.root.join("blobs/ab/abhash"));
    let b = unit(&units, "models--org--b");
    let c = unit(&units, "models--org--c");
    assert_eq!(
        b.bytes,
        idx.get(&h.root.join("models--org--b"))
            .unwrap()
            .allocated_total
            + shared
    );
    assert!(text_of(b).contains("moving this repo folder leaves those bytes there"));
    assert!(text_of(c).contains("counted under another repo"));
    let blobs = unit(&units, "blobs");
    assert!(
        text_of(blobs).contains("1 file(s)")
            && text_of(blobs).contains("not referenced by any repo's blobs/"),
        "{blobs:?}"
    );
    assert!(ev(a, REVISION_EVIDENCE)[0].contains("main -> 11111111"));
    assert!(ev(a, REVISION_EVIDENCE)[0].contains("2 revision(s)"));
}

/// Tempting wrong patch: resolving links with the filesystem (following
/// them), which reads `/etc/hosts` through a snapshot and counts it.
#[test]
fn dangling_escaping_and_incomplete_are_reported_facts() {
    let h = hub_fixture(README);
    let (units, _) = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    );
    let a = text_of(unit(&units, "models--org--a"));
    assert!(
        a.contains("1 snapshot link(s) point at a blob that is not there"),
        "{a}"
    );
    assert!(a.contains("point outside this repo's blobs/"), "{a}");
    assert!(
        a.contains("incomplete download: 1 .incomplete file(s)"),
        "{a}"
    );
}

#[test]
fn the_card_says_what_the_files_say_and_nothing_else() {
    let h = hub_fixture(README);
    let (units, _) = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    );
    let a = unit(&units, "models--org--a");
    assert_eq!(
        ev(a, CARD_EVIDENCE),
        vec![
            "text-generation · llama · 1.0M params · bfloat16 · license llama3 · base meta-llama/Llama-3.1-8B"
        ]
    );
    assert!(ev(a, FIELD_EVIDENCE).contains(&"params: 1001000"));
    assert!(ev(a, FIELD_EVIDENCE).contains(&"tokenizer: present"));
    let card = ev(a, CARD_TEXT_EVIDENCE)[0];
    assert_eq!(card, "A small test model[2J for fixtures.");
    let tuned = unit(&units, "models--me--tuned");
    assert!(
        ev(tuned, CARD_EVIDENCE).is_empty(),
        "no field invented: {tuned:?}"
    );
    assert_eq!(
        tuned.consequence.as_deref(),
        Some("cannot be regenerated (no source recorded)")
    );
    let ac = a.consequence.clone().unwrap();
    assert!(
        ac.starts_with("downloaded again from huggingface.co (org/a@11111111) when needed; size "),
        "{ac}"
    );
    assert_eq!(
        crate::reclaim::class_from_consequence(&ac),
        crate::locations::RegenClass::Download
    );
}

/// Tempting wrong patch: trusting the 8-byte length and reading (or
/// allocating) what it claims; or treating malformed JSON as zero
/// parameters.
#[test]
fn hostile_headers_and_malformed_files_give_explicit_unknowns() {
    let h = hub_fixture("---\nlicense: [unclosed\n");
    let a = h.root.join("models--org--a/blobs");
    let mut evil = u64::MAX.to_le_bytes().to_vec();
    evil.extend(b"{}");
    fs::write(a.join("h-weights"), evil).unwrap();
    fs::write(a.join("h-config"), "{ not json").unwrap();
    let (units, w) = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    );
    let u = unit(&units, "models--org--a");
    assert!(
        !ev(u, FIELD_EVIDENCE)
            .iter()
            .any(|f| f.starts_with("params:"))
    );
    assert!(
        ev(u, FIELD_EVIDENCE)
            .iter()
            .any(|f| f.contains("over the 1 MiB read bound")),
        "{u:?}"
    );
    assert!(
        !ev(u, FIELD_EVIDENCE)
            .iter()
            .any(|f| f.starts_with("model_type"))
    );
    // Every read is bounded: no single header read past the cap.
    assert!(w.header_bytes_read <= 8 * model_cards::MAX_CARD_READ as u64);
}

/// Tempting wrong patch (the maintainer's pass-through rule): parsing
/// on every pass. The second pass over an unchanged cache reads zero
/// content bytes; a new revision re-parses only that repo.
#[test]
fn an_unchanged_cache_reads_nothing_and_a_new_revision_reparses_only_itself() {
    let h = hub_fixture(README);
    let cards = CardCache::default();
    let (_, cold) = identify(BuildStoreKind::HuggingFaceHub, &h.root, &cards);
    assert!(cold.header_bytes_read > 0);
    let warm_cards = CardCache::from_entries(cards.retained(&|_| false), 64);
    let (units, warm) = identify(BuildStoreKind::HuggingFaceHub, &h.root, &warm_cards);
    assert_eq!(warm.header_bytes_read, 0, "warm pass read content");
    assert_eq!(warm.subprocess_spawns, 0);
    assert_eq!(
        ev(unit(&units, "models--org--a"), CARD_EVIDENCE).len(),
        1,
        "the hit still gives the card"
    );
    // A new revision of org/c only.
    let c = h.root.join("models--org--c");
    let snap = c.join("snapshots").join(REV2);
    fs::create_dir_all(&snap).unwrap();
    symlink("../../blobs/hb", snap.join("model.safetensors")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(c.join("refs/main"), REV2).unwrap();
    let third = CardCache::from_entries(warm_cards.retained(&|_| false), 64);
    let (_, w3) = identify(BuildStoreKind::HuggingFaceHub, &h.root, &third);
    assert!(w3.header_bytes_read > 0);
    let kept = third.retained(&|_| false);
    assert!(kept.contains_key(&hub_card_key(&h.root, "models--org--c", REV2)));
    assert!(
        !kept.contains_key(&hub_card_key(&h.root, "models--org--c", REV3)),
        "the old revision's entry is pruned once nothing uses it"
    );
    let a_key = hub_card_key(&h.root, "models--org--a", REV1);
    assert_eq!(kept[&a_key].at, 1_000_000);
}

/// Tempting wrong patch: no cap, so a cold cache of hundreds of repos
/// reads hundreds of weight headers in one observe.
#[test]
fn new_card_reads_are_capped_per_pass() {
    let h = hub_fixture(README);
    let cards = CardCache::from_entries(HashMap::new(), 1);
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &h.root, &cards);
    let not_yet = units
        .iter()
        .filter(|u| text_of(u).contains("what it is: not yet read"))
        .count();
    let read = units
        .iter()
        .filter(|u| !ev(u, CARD_EVIDENCE).is_empty())
        .count();
    assert_eq!(read, 1, "{units:#?}");
    assert!(not_yet >= 2);
}

/// Tempting wrong patch: reporting the weight file's access time as is,
/// which after swamp's own header read is swamp's read.
#[test]
fn swamps_own_header_read_is_not_reported_as_use() {
    let h = hub_fixture(README);
    let cards = CardCache::default();
    let _ = identify(BuildStoreKind::HuggingFaceHub, &h.root, &cards);
    let key = hub_card_key(&h.root, "models--org--a", REV1);
    let mut entry = cards.peek(&key).unwrap();
    // Pretend the access time before swamp's read was long ago.
    entry
        .fields
        .insert("read_atime_before".into(), "1000".into());
    let after = {
        use std::os::unix::fs::MetadataExt;
        crate::fs_gate::symlink_metadata(h.root.join("models--org--a/blobs/h-weights"))
            .unwrap()
            .atime()
    };
    entry
        .fields
        .insert("read_atime_after".into(), after.to_string());
    let mut stored = cards.retained(&|_| false);
    stored.insert(key, entry);
    let (units, _) = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::from_entries(stored, 64),
    );
    let lr = ev(unit(&units, "models--org--a"), LAST_READ_EVIDENCE)[0];
    assert_eq!(
        lr,
        "1000|file access time of model.safetensors, before swamp's own header read"
    );
}

struct Ollama {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

/// Two tags sharing their model layer, one blob no manifest names, a
/// partial download and a malformed manifest.
fn ollama_fixture() -> Ollama {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("models");
    let blobs = root.join("blobs");
    let lib = root.join("manifests/registry.ollama.ai/library");
    fs::create_dir_all(&blobs).unwrap();
    fs::create_dir_all(lib.join("qwen3")).unwrap();
    fs::create_dir_all(lib.join("broken")).unwrap();
    let write_blob = |c: char, body: &[u8]| {
        fs::write(blobs.join(digest(c).replacen(':', "-", 1)), body).unwrap();
    };
    write_blob('a', &vec![7u8; 40_000]);
    write_blob(
        'c',
        br#"{"model_format":"gguf","model_family":"qwen3","model_type":"751.63M","file_type":"Q4_K_M"}"#,
    );
    write_blob('d', br#"{"model_format":"gguf","model_family":"qwen3","model_type":"751.63M","file_type":"Q8_0"}"#);
    write_blob('e', &vec![1u8; 9_000]);
    fs::write(
        blobs.join(format!("{}-partial", digest('f').replacen(':', "-", 1))),
        vec![0u8; 3000],
    )
    .unwrap();
    let manifest = |config: char| {
        format!(
            r#"{{"schemaVersion":2,"config":{{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"{}","size":90}},"layers":[{{"mediaType":"application/vnd.ollama.image.model","digest":"{}","size":40000}}]}}"#,
            digest(config),
            digest('a')
        )
    };
    fs::write(lib.join("qwen3/0.6b"), manifest('c')).unwrap();
    fs::write(lib.join("qwen3/q8"), manifest('d')).unwrap();
    fs::write(lib.join("broken/latest"), "{ nope").unwrap();
    Ollama { _tmp: tmp, root }
}

/// Tempting wrong patch: charging a shared layer to every tag that
/// names it (two tags of one model would show twice its size), or
/// calling the loose blob something it is not.
#[test]
fn ollama_shared_layers_are_counted_once_and_loose_blobs_are_a_fact() {
    let o = ollama_fixture();
    let (units, _) = identify(BuildStoreKind::OllamaModels, &o.root, &CardCache::default());
    let a = allocated(&o.root.join("blobs").join(digest('a').replacen(':', "-", 1)));
    let first = unit(&units, "0.6b");
    let second = unit(&units, "q8");
    assert!(first.bytes >= a, "{first:?}");
    assert!(
        second.bytes < a,
        "the shared layer is charged once: {second:?}"
    );
    assert!(text_of(second).contains("counted under another model above"));
    assert!(text_of(first).contains("shared with other models"));
    assert_eq!(
        ev(first, CARD_EVIDENCE),
        vec!["qwen3 · 751.63M params · Q4_K_M"]
    );
    assert!(
        first
            .consequence
            .as_deref()
            .unwrap()
            .starts_with("downloaded again with `ollama pull qwen3:0.6b` when needed")
    );
    assert!(text_of(first).contains("`ollama rm qwen3:0.6b`"));
    let blobs = unit(&units, "blobs");
    assert!(
        text_of(blobs).contains("1 blob(s)")
            && text_of(blobs).contains("not referenced by any manifest"),
        "{blobs:?}"
    );
    let root = units.iter().find(|u| u.path == o.root).unwrap();
    assert!(text_of(root).contains("incomplete download: 1 partial blob file(s)"));
    let broken = unit(&units, "latest");
    assert!(
        text_of(broken).contains("not one this reader understands"),
        "{broken:?}"
    );
    let sum: u64 = units
        .iter()
        .filter(|u| u.path != o.root)
        .map(|u| u.bytes)
        .sum();
    let blobs_total = folded(&o.root)
        .get(&o.root.join("blobs"))
        .unwrap()
        .allocated_total;
    assert_eq!(
        sum, blobs_total,
        "tags plus blobs/ residual equal the blob folder"
    );
}

#[test]
fn an_unchanged_ollama_store_reads_no_manifest_twice() {
    let o = ollama_fixture();
    let cards = CardCache::default();
    let (_, cold) = identify(BuildStoreKind::OllamaModels, &o.root, &cards);
    assert!(cold.header_bytes_read > 0);
    let warm = CardCache::from_entries(cards.retained(&|_| false), 64);
    let (_, w) = identify(BuildStoreKind::OllamaModels, &o.root, &warm);
    // The malformed manifest is re-read each pass (it produced no entry);
    // nothing else is.
    assert_eq!(w.header_bytes_read, "{ nope".len() as u64);
}

/// Tempting wrong patch: a verdict word or an em dash in the new
/// strings (the repo's grep gate and the renderer's panic).
#[test]
fn no_unit_text_carries_a_verdict_word_or_em_dash() {
    let h = hub_fixture(README);
    let o = ollama_fixture();
    let mut all = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    )
    .0;
    all.extend(identify(BuildStoreKind::OllamaModels, &o.root, &CardCache::default()).0);
    let banned = [
        concat!("un", "used"),
        concat!("obso", "lete"),
        concat!("st", "ale"),
        concat!("orph", "an"),
        concat!("sa", "fe"),
    ];
    for u in &all {
        let t = text_of(u).to_ascii_lowercase();
        assert!(!t.contains('\u{2014}'), "{t}");
        for w in t.split(|c: char| !c.is_ascii_alphanumeric()) {
            assert!(!banned.contains(&w), "{w} in {t}");
        }
        assert!(
            matches!(
                u.action,
                crate::artifact::NestedActionCapability::Unsupported { .. }
            ),
            "{u:?}"
        );
    }
}

/// Not a test: the numbers in the CHANGELOG. Read-only over this
/// machine's real caches (`HF_HUB_CACHE`/`~/.cache/huggingface/hub`,
/// `OLLAMA_MODELS`/`~/.ollama/models`); prints sizes, the "what it is"
/// lines, and the cold and warm identification times.
/// `cargo test -p swamp-core --lib -- --ignored measure_real_model_caches --nocapture`
#[test]
#[ignore]
fn measure_real_model_caches() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    for (kind, root) in [
        (
            BuildStoreKind::HuggingFaceHub,
            home.join(".cache/huggingface/hub"),
        ),
        (BuildStoreKind::OllamaModels, home.join(".ollama/models")),
    ] {
        if crate::fs_gate::symlink_metadata(&root).is_err() {
            continue;
        }
        let cards = CardCache::default();
        let t = std::time::Instant::now();
        let (units, cold) = identify(kind, &root, &cards);
        let cold_t = t.elapsed();
        let warm_cards = CardCache::from_entries(cards.retained(&|_| false), 64);
        let t = std::time::Instant::now();
        let (_, warm) = identify(kind, &root, &warm_cards);
        let warm_t = t.elapsed();
        let total = folded(&root).get(&root).map_or(0, |d| d.allocated_total);
        println!(
            "{}: store {} B; cold {:?} ({} header bytes, {} stats, {} listings); warm {:?} ({} header bytes, {} stats)",
            root.display(),
            total,
            cold_t,
            cold.header_bytes_read,
            cold.files_statted,
            cold.dirs_listed,
            warm_t,
            warm.header_bytes_read,
            warm.files_statted
        );
        for m in model_rows(&root, &units, crate::entities::now()) {
            println!(
                "  {} {:?} {} B | {} | last read {} | {}",
                m.name,
                m.revision,
                m.bytes,
                m.about.as_deref().unwrap_or("-"),
                m.last_read,
                m.regeneration
            );
            for f in &m.facts {
                println!("      {f}");
            }
        }
        for u in units.iter().filter(|u| u.role == ArtifactRole::Residual) {
            println!(
                "  [{}] {} B {:?}",
                u.path.display(),
                u.bytes,
                u.coverage.limits
            );
        }
    }
}

/// Docs match code: every bound `docs/usage.md` states for model caches
/// is the constant the code uses. Tempting wrong patch: changing a cap
/// in code and leaving the documented number behind.
#[test]
fn usage_doc_states_the_bounds_the_code_uses() {
    let doc = include_str!("../../../../../docs/usage.md");
    let section =
        &doc[doc.find("### Model caches").unwrap()..doc.find("### The Reclaim view").unwrap()];
    for needle in [
        format!("At most {}\n  new parses", model_cards::NEW_PARSES_PER_PASS),
        format!("at most {} characters", model_cards::MAX_CARD_TEXT_CHARS),
        format!(
            "At most {} requests per observe",
            crate::hub_api::MAX_FETCHES_PER_PASS
        ),
        format!(
            "fetched again after {} days",
            crate::hub_api::MUTABLE_TTL_SECS / 86_400
        ),
        "kept for a day".to_string(),
        format!("{} MiB at most", model_cards::MAX_CARD_READ / (1024 * 1024)),
        "`associations/model_cards.parquet`".to_string(),
        "`hf_enrich = true`".to_string(),
    ] {
        assert!(
            section.contains(&needle),
            "docs/usage.md Model caches lacks {needle:?}"
        );
    }
    assert_eq!(crate::hub_api::NEGATIVE_TTL_SECS, 86_400);
    assert!(crate::hub_api::OFF_LINE.contains("`hf_enrich = true`"));
    assert!(
        doc.contains("hf_enrich = false"),
        "the config block names the key"
    );
    assert!(
        crate::growth::GrowthConfig::default()
            .to_toml()
            .contains("hf_enrich = false")
    );
}
