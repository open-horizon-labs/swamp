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

/// Marks a file as read since its last change (access time after its
/// mtime), as a model a program has loaded is: only then does swamp read
/// its header without moving the access time it reports.
fn used(p: &Path) {
    use std::os::unix::fs::MetadataExt;
    let m = fs::symlink_metadata(p).unwrap();
    let at = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs((m.mtime().max(m.ctime()) + 60) as u64);
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(at))
        .unwrap();
}

/// Allocated bytes as the filesystem reports them once every earlier
/// write is reflected (ZFS assigns `st_blocks` at commit; #197).
fn allocated(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    crate::fs_gate::settle::settle();
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
    used(&a.join("blobs/h-weights"));
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
    used(&root.join("blobs/ab/abhash"));
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
    crate::fs_gate::settle::settle();
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
    // The adapter stats blobs; on ZFS their blocks appear at commit.
    crate::fs_gate::settle::settle();
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
    assert!(text_of(b).contains("stays in the hub's shared blobs/"));
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
    used(&a.join("h-weights"));
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
    // The shared layer dominates every other file (incompressible, 1 MiB),
    // so no filesystem's block size or compression can blur which tag it
    // is charged to.
    write_blob('a', &crate::fs_gate::settle::noise(1024 * 1024));
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
    crate::fs_gate::settle::settle();
    Ollama { _tmp: tmp, root }
}

/// Tempting wrong patch: charging a shared layer to every tag that
/// names it (two tags of one model would show twice its size), or
/// calling the loose blob something it is not.
#[test]
fn ollama_shared_layers_are_counted_once_and_loose_blobs_are_a_fact() {
    let o = ollama_fixture();
    let (units, _) = identify(BuildStoreKind::OllamaModels, &o.root, &CardCache::default());
    let blob = |c: char| allocated(&o.root.join("blobs").join(digest(c).replacen(':', "-", 1)));
    let first = unit(&units, "0.6b");
    let second = unit(&units, "q8");
    // Exact, from the files' own allocation: the first tag carries the
    // shared layer and its config; the second only its own config.
    assert_eq!(first.bytes, blob('a') + blob('c'), "{first:?}");
    assert_eq!(
        second.bytes,
        blob('d'),
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
    // The malformed manifest's failure is cached too (size and mtime).
    assert_eq!(w.header_bytes_read, 0);
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

// ---------------------------------------------------------------------
// Adversarial audit (audit/v080-g7). Each test names the failure it
// demonstrates; a FAILING test here is a must-fix finding.
// ---------------------------------------------------------------------

fn one_repo(root: &Path, folder: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let r = root.join(folder);
    fs::create_dir_all(r.join("blobs")).unwrap();
    fs::create_dir_all(r.join("refs")).unwrap();
    let snap = r.join("snapshots").join(REV1);
    fs::create_dir_all(&snap).unwrap();
    fs::write(r.join("refs/main"), REV1).unwrap();
    for (i, (name, body)) in files.iter().enumerate() {
        let blob = format!("b{i}");
        fs::write(r.join("blobs").join(&blob), body).unwrap();
        if let Some(parent) = Path::new(name).parent() {
            fs::create_dir_all(snap.join(parent)).unwrap();
        }
        let depth = name.matches('/').count();
        let up = "../".repeat(2 + depth);
        symlink(format!("{up}blobs/{blob}"), snap.join(name)).unwrap();
    }
    r
}

fn field<'a>(u: &'a NestedArtifact, key: &str) -> Option<&'a str> {
    ev(u, FIELD_EVIDENCE)
        .into_iter()
        .find_map(|f| f.strip_prefix(&format!("{key}: ")))
}

/// ADV-1 (must-fix). A model sharded into 17 safetensors files: the
/// adapter reads 16 headers (MAX_WEIGHT_HEADERS) and still states the
/// sum of those 16 as `params` "exact". 17 shards of one parameter each
/// must give 17 or no exact count; it gives 16.
#[test]
fn adv_params_over_more_shards_than_the_header_cap_is_not_exact() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let files: Vec<(String, Vec<u8>)> = (1..=17)
        .map(|i| {
            (
                format!("model-{i:05}-of-00017.safetensors"),
                st_header(&[("w", "BF16", &[1])]),
            )
        })
        .collect();
    let files: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
    one_repo(&root, "models--org--big", &files);
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let u = unit(&units, "models--org--big");
    let p = field(u, "params");
    assert!(
        p.is_none() || p == Some("17"),
        "params {p:?} stated exact from 16 of 17 shards: {:?}",
        ev(u, FIELD_EVIDENCE)
    );
}

/// ADV-2 (must-fix). Mistral-style repos ship the same weights twice:
/// `consolidated.safetensors` and `model-0000N-of-0000M.safetensors`.
/// Summing every safetensors header in the snapshot doubles the count
/// and labels it "exact".
#[test]
fn adv_params_are_not_summed_across_duplicate_weight_formats() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    one_repo(
        &root,
        "models--mistralai--m",
        &[
            (
                "consolidated.safetensors",
                st_header(&[("w", "BF16", &[100, 10])]),
            ),
            (
                "model-00001-of-00002.safetensors",
                st_header(&[("a", "BF16", &[50, 10])]),
            ),
            (
                "model-00002-of-00002.safetensors",
                st_header(&[("b", "BF16", &[50, 10])]),
            ),
        ],
    );
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let u = unit(&units, "models--mistralai--m");
    assert_ne!(
        field(u, "params"),
        Some("2000"),
        "the same 1000 parameters counted twice and called exact: {:?}",
        ev(u, FIELD_EVIDENCE)
    );
}

/// ADV-3 (must-fix). A repo's `blobs/` that is itself a symlink out of
/// the cache. `list_links` (read_dir) and `lstat` follow a symlinked
/// *parent*, and `fs_gate::read::open_regular` follows symlinks (no
/// O_NOFOLLOW), so a snapshot link `README.md -> ../../blobs/secret`
/// reads a file outside the hub and shows its text as the model card.
#[test]
fn adv_a_symlinked_repo_blobs_dir_is_never_read_through() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret"), "TOPSECRET private text\n").unwrap();
    let r = root.join("models--org--evil");
    fs::create_dir_all(r.join("refs")).unwrap();
    fs::create_dir_all(r.join("snapshots").join(REV1)).unwrap();
    fs::write(r.join("refs/main"), REV1).unwrap();
    symlink(&outside, r.join("blobs")).unwrap();
    symlink(
        "../../blobs/secret",
        r.join("snapshots").join(REV1).join("README.md"),
    )
    .unwrap();
    let (units, w) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let t = text_of(unit(&units, "models--org--evil"));
    assert!(!t.contains("TOPSECRET"), "read outside the cache: {t}");
    assert_eq!(
        w.header_bytes_read, 40,
        "opened a file outside the cache (40 = the ref)"
    );
}

/// ADV-4 (must-fix). A sub-folder of the hub's shared `blobs/` that is a
/// symlink out of the cache: the lexical check passes
/// (`hub/blobs/ab/x` starts with `hub/blobs`), `lstat` follows the
/// symlinked parent, and the outside file's bytes are charged to the
/// repo, so the units no longer sum to the store.
#[test]
fn adv_a_symlinked_shared_blob_subdir_is_not_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("big"), vec![9u8; 200_000]).unwrap();
    fs::create_dir_all(root.join("blobs")).unwrap();
    symlink(&outside, root.join("blobs/ab")).unwrap();
    let r = root.join("models--org--x");
    fs::create_dir_all(r.join("blobs")).unwrap();
    symlink("../../blobs/ab/big", r.join("blobs/hb")).unwrap();
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let store_total = folded(&root).get(&root).unwrap().allocated_total;
    let sum: u64 = units
        .iter()
        .filter(|u| u.path != root)
        .map(|u| u.bytes)
        .sum();
    assert_eq!(
        sum, store_total,
        "bytes outside the cache counted: {units:#?}"
    );
}

/// ADV-5. A link inside a repo's own `blobs/` to a sibling blob is put in
/// `shared` and charged again on top of the folder's walked total.
#[test]
fn adv_a_link_between_a_repos_own_blobs_is_counted_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let r = one_repo(
        &root,
        "models--org--y",
        &[("model.safetensors", st_header(&[("w", "F32", &[4])]))],
    );
    symlink("b0", r.join("blobs/alias")).unwrap();
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let store_total = folded(&root).get(&root).unwrap().allocated_total;
    let sum: u64 = units
        .iter()
        .filter(|u| u.path != root)
        .map(|u| u.bytes)
        .sum();
    assert_eq!(sum, store_total, "{units:#?}");
}

/// ADV-6. Ollama `blobs/` as a symlink out of the store: listed and
/// statted through, charged to the models, while the walk counted
/// nothing there.
#[test]
fn adv_a_symlinked_ollama_blobs_dir_is_not_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("models");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join(digest('a').replacen(':', "-", 1)),
        vec![7u8; 100_000],
    )
    .unwrap();
    let lib = root.join("manifests/registry.ollama.ai/library/m");
    fs::create_dir_all(&lib).unwrap();
    symlink(&outside, root.join("blobs")).unwrap();
    fs::write(
        lib.join("latest"),
        format!(
            r#"{{"config":{{"mediaType":"c","digest":"{}","size":1}},"layers":[{{"mediaType":"application/vnd.ollama.image.model","digest":"{}","size":100000}}]}}"#,
            digest('c'),
            digest('a')
        ),
    )
    .unwrap();
    let (units, _) = identify(BuildStoreKind::OllamaModels, &root, &CardCache::default());
    let m = unit(&units, "latest");
    assert_eq!(
        m.bytes, 0,
        "bytes outside the store charged to a model: {m:?}"
    );
}

/// ADV-7. A repo folder name with a right-to-left override reaches the
/// unit's package name and consequence unstripped (folder names come
/// from the disk, not from swamp).
#[test]
fn adv_bidi_in_a_repo_folder_name_is_stripped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    one_repo(
        &root,
        "models--org--a\u{202E}gpj.exe",
        &[("config.json", br#"{"model_type":"x"}"#.to_vec())],
    );
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let rows = model_rows(&root, &units, 2_000_000_000);
    let json = serde_json::to_value(&rows).unwrap().to_string();
    assert!(!json.contains('\u{202E}'), "{json}");
}

/// ADV-8. A malformed Ollama manifest is not cached, so every pass
/// re-reads it and spends one unit of the 64-parse budget on it: 64
/// broken manifests starve every new model of its read, forever.
#[test]
fn adv_malformed_manifests_do_not_starve_the_parse_budget() {
    let o = ollama_fixture();
    let lib = o.root.join("manifests/registry.ollama.ai/library");
    for i in 0..model_cards::NEW_PARSES_PER_PASS {
        fs::create_dir_all(lib.join(format!("aa{i:03}"))).unwrap();
        fs::write(lib.join(format!("aa{i:03}/latest")), "{ nope").unwrap();
    }
    let cards = CardCache::default();
    let _ = identify(BuildStoreKind::OllamaModels, &o.root, &cards);
    let mut prev = cards.retained(&|_| false);
    for _ in 0..3 {
        let c = CardCache::from_entries(prev, model_cards::NEW_PARSES_PER_PASS);
        let (units, _) = identify(BuildStoreKind::OllamaModels, &o.root, &c);
        prev = c.retained(&|_| false);
        let q = unit(&units, "0.6b");
        if !ev(q, CARD_EVIDENCE).is_empty() {
            return;
        }
    }
    panic!("after 4 passes qwen3:0.6b is still 'not yet read'");
}

/// ADV-9 (documents, passes). The manifest fingerprint is size + mtime
/// (seconds and nanoseconds). A content swap that keeps both (only
/// `touch -r` or a deliberate restore does) is answered from the cache.
/// Ollama writes manifests by rename with a fresh mtime, so this does
/// not arise in practice.
#[test]
fn adv_same_size_same_mtime_manifest_swap_is_answered_from_the_cache() {
    let o = ollama_fixture();
    let cards = CardCache::default();
    let _ = identify(BuildStoreKind::OllamaModels, &o.root, &cards);
    let p = o
        .root
        .join("manifests/registry.ollama.ai/library/qwen3/0.6b");
    let before = fs::metadata(&p).unwrap().modified().unwrap();
    let text = fs::read_to_string(&p)
        .unwrap()
        .replace(&digest('c'), &digest('d'));
    fs::write(&p, text).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(before)
        .unwrap();
    let warm = CardCache::from_entries(cards.retained(&|_| false), 64);
    let (units, _) = identify(BuildStoreKind::OllamaModels, &o.root, &warm);
    assert_eq!(
        ev(unit(&units, "0.6b"), CARD_EVIDENCE),
        vec!["qwen3 · 751.63M params · Q4_K_M"]
    );
}

/// ADV-10 (holds?). A snapshot dir that is a symlink, and a snapshot
/// link to a FIFO in blobs: neither opened (a FIFO read would hang).
#[test]
fn adv_fifo_blob_and_symlinked_snapshot_are_not_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let r = one_repo(&root, "models--org--f", &[]);
    let fifo = r.join("blobs/fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    symlink(
        "../../blobs/fifo",
        r.join("snapshots").join(REV1).join("README.md"),
    )
    .unwrap();
    symlink("/etc", r.join("snapshots").join(REV2)).unwrap();
    let t0 = std::time::Instant::now();
    let (units, w) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    assert!(t0.elapsed() < std::time::Duration::from_secs(2));
    // The 40-byte ref only; the FIFO is never opened.
    assert_eq!(w.header_bytes_read, 40);
    assert!(!text_of(unit(&units, "models--org--f")).contains("22222222"));
}

/// ADV-11 (holds?). Hostile GGUF and YAML bounded in time.
#[test]
fn adv_hostile_gguf_and_front_matter_are_bounded() {
    // GGUF: 2^20 kvs claimed, each an array of 2^60 nested arrays.
    let mut b = b"GGUF".to_vec();
    b.extend(3u32.to_le_bytes());
    b.extend(u64::MAX.to_le_bytes());
    b.extend(u64::MAX.to_le_bytes());
    for _ in 0..40_000 {
        b.extend(1u64.to_le_bytes());
        b.extend(b"k");
        b.extend(9u32.to_le_bytes());
        b.extend(9u32.to_le_bytes());
        b.extend((1u64 << 60).to_le_bytes());
    }
    b.truncate(model_cards::MAX_CARD_READ);
    let t0 = std::time::Instant::now();
    let _ = model_cards::gguf_header(&b);
    // Billion laughs and a 1 MiB single line.
    let mut y = String::from("---\nlicense: &a [x,x,x,x,x,x,x,x,x]\n");
    for i in 0..20_000 {
        y.push_str(&format!("tags: &l{i} [*a,*a,*a,*a,*a,*a,*a]\n"));
    }
    y.push_str(&"z".repeat(1 << 20));
    y.push_str("\n---\n");
    y.push_str(&"p".repeat(1 << 20));
    let f = model_cards::front_matter(&y);
    let p = model_cards::first_paragraph(&y);
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        t0.elapsed()
    );
    assert!(f.values().all(|v| v.chars().count() <= 120));
    assert!(p.is_none_or(|p| p.chars().count() <= model_cards::MAX_CARD_TEXT_CHARS));
}

/// ADV-12 (must-fix; seen on the real binary). `model_rows` matches
/// `i.path.starts_with(unit_path)`, so the parent unit `~/.cache/huggingface`
/// (770 KB, local-state) lists the hub's 968.9 MB models as its own in
/// `report --view reclaim`. Rows belong to the store they were identified
/// in (the container), not every ancestor. `attach_last_read` has the same
/// prefix match.
#[test]
fn adv_model_rows_belong_to_their_store_not_its_ancestors() {
    let h = hub_fixture(README);
    let (units, _) = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    );
    let parent = h.root.parent().unwrap();
    let rows = model_rows(parent, &units, 2_000_000_000);
    assert!(
        rows.is_empty(),
        "the HF_HOME unit lists the hub's models: {}",
        rows.len()
    );
}

/// ADV-13 (real APFS behaviour). The own-read correction lives only in
/// the card cache. When that cache is gone (a store-format reset deletes
/// `model_cards.parquet`, or any re-parse), the next pass records
/// swamp's *previous* header read as `read_atime_before`, and reports
/// swamp's own read as the weight file's last read.
#[test]
fn adv_after_a_cache_reset_swamps_earlier_read_is_not_shown_as_use() {
    let h = hub_fixture(README);
    let w = h.root.join("models--org--a/blobs/h-weights");
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    fs::File::options()
        .write(true)
        .open(&w)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(old))
        .unwrap();
    let _ = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    );
    let moved = {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(&w).unwrap().atime()
    };
    // Changed in the fix (reported): swamp no longer reads a weight file
    // whose access time a read would move, so the precondition the audit
    // relied on (APFS moving it) no longer occurs. The stronger property
    // is asserted: the access time did not move.
    assert_eq!(moved, 1_500_000_000, "swamp's read moved the access time");
    // Cache gone (store-format reset): a fresh pass.
    let (units, _) = identify(
        BuildStoreKind::HuggingFaceHub,
        &h.root,
        &CardCache::default(),
    );
    let lr = ev(unit(&units, "models--org--a"), LAST_READ_EVIDENCE)[0];
    // The time shown is the access time from before any swamp pass (the
    // only real use this fixture has), not a time a swamp read wrote.
    assert!(
        lr.starts_with("1500000000|"),
        "swamp's own earlier read shown as the last read: {lr}"
    );
}

/// Tempting wrong patch: reading the weight header whatever its access
/// time, so swamp's own read becomes the "last read" (APFS moves an
/// access time not newer than the last change). Such a file is not read:
/// no count yet, a stated reason, and its access time is unchanged.
#[test]
fn a_weight_file_whose_access_time_a_read_would_move_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let r = one_repo(
        &root,
        "models--org--fresh",
        &[("model.safetensors", st_header(&[("w", "F32", &[10])]))],
    );
    let w = r.join("blobs/b0");
    let before = {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(&w).unwrap().atime()
    };
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let u = unit(&units, "models--org--fresh");
    if cfg!(target_os = "macos") {
        assert_eq!(field(u, "params"), None);
        assert!(field(u, "params_deferred").is_some_and(|v| v.starts_with("not read yet")));
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.starts_with("parameter count not read yet"))
        );
        assert!(text_of(u).contains("reading the weight file now would set its access time"));
    }
    let after = {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(&w).unwrap().atime()
    };
    assert_eq!(before, after);
    // Once a program has read it, the count is read (from the cache's
    // deferred entry: a miss, not a stale answer).
    used(&w);
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    assert_eq!(
        field(unit(&units, "models--org--fresh"), "params"),
        Some("10")
    );
}

/// The audit's ADV-1 and ADV-2 with weight files a program has read, so
/// the counts are actually computed. Tempting wrong patch: summing every
/// safetensors file, or stating 16 of 17 shards as exact.
#[test]
fn weight_sets_count_one_copy_and_never_a_partial_set_as_exact() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let r = one_repo(
        &root,
        "models--mistralai--m",
        &[
            (
                "consolidated.safetensors",
                st_header(&[("w", "BF16", &[100, 10])]),
            ),
            (
                "model-00001-of-00002.safetensors",
                st_header(&[("a", "BF16", &[50, 10])]),
            ),
            (
                "model-00002-of-00002.safetensors",
                st_header(&[("b", "BF16", &[50, 10])]),
            ),
        ],
    );
    for i in 0..3 {
        used(&r.join(format!("blobs/b{i}")));
    }
    let r2 = one_repo(
        &root,
        "models--org--idx",
        &[
            ("model.safetensors.index.json", br#"{"weight_map":{"a":"model-00001-of-00002.safetensors","b":"model-00002-of-00002.safetensors"}}"#.to_vec()),
            ("model-00001-of-00002.safetensors", st_header(&[("a", "F16", &[7])])),
            ("model-00002-of-00002.safetensors", st_header(&[("b", "F16", &[5])])),
            ("model.fp16.safetensors", st_header(&[("x", "F16", &[12])])),
        ],
    );
    for i in 1..4 {
        used(&r2.join(format!("blobs/b{i}")));
    }
    let files: Vec<(String, Vec<u8>)> = (1..=17)
        .map(|i| {
            (
                format!("model-{i:05}-of-00017.safetensors"),
                st_header(&[("w", "BF16", &[1])]),
            )
        })
        .collect();
    let files: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
    let r3 = one_repo(&root, "models--org--big", &files);
    for i in 0..17 {
        used(&r3.join(format!("blobs/b{i}")));
    }
    let r4 = one_repo(
        &root,
        "models--org--gap",
        &[(
            "model-00001-of-00003.safetensors",
            st_header(&[("a", "F16", &[7])]),
        )],
    );
    used(&r4.join("blobs/b0"));
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let m = unit(&units, "models--mistralai--m");
    assert_eq!(
        field(m, "params"),
        Some("1000"),
        "{:?}",
        ev(m, FIELD_EVIDENCE)
    );
    assert!(
        field(m, "params_note")
            .unwrap()
            .contains("one of 2 weight sets")
    );
    assert_eq!(
        field(unit(&units, "models--org--idx"), "params"),
        Some("12")
    );
    let big = unit(&units, "models--org--big");
    assert_eq!(field(big, "params"), None);
    assert!(
        field(big, "params_unread")
            .unwrap()
            .contains("17 weight files")
    );
    let gap = unit(&units, "models--org--gap");
    assert_eq!(field(gap, "params"), None);
    assert_eq!(field(gap, "params_unread"), Some("1 of 3 shards are here"));
}

/// REV2-1 (audit/v080-g7b). A repo with no weight file (a README and a
/// config only: a partial `--include` download, a dataset card): the
/// last-read fallback is the largest blob, which is the README swamp
/// itself reads for the card. On APFS a fresh file's access time is not
/// newer than its change, so that read moves it, and swamp's own read is
/// shown as the user's last use. Tempting wrong patch: guarding only the
/// weight file with `read_would_move_atime` while the fallback still
/// takes the atime of any blob, including ones swamp just read.
#[test]
fn rev2_a_readme_swamp_reads_is_not_shown_as_last_use() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let mut readme = String::from("---\nlicense: mit\n---\n\nA card.\n");
    readme.push_str(&"x".repeat(20_000));
    let r = one_repo(
        &root,
        "models--org--cardonly",
        &[
            ("README.md", readme.into_bytes()),
            ("config.json", br#"{"model_type":"bert"}"#.to_vec()),
        ],
    );
    let readme_blob = r.join("blobs/b0");
    // Downloaded long ago, never opened since (atime before mtime).
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    fs::File::options()
        .write(true)
        .open(&readme_blob)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(old))
        .unwrap();
    // First pass reads the README for the card; a second pass (cache
    // reset, or any re-parse) then takes the last read from the blobs.
    let _ = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let after = fs::symlink_metadata(&readme_blob).unwrap().atime();
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let lr = ev(unit(&units, "models--org--cardonly"), LAST_READ_EVIDENCE);
    assert!(
        after == 1_500_000_000 || lr.iter().all(|l| !l.starts_with(&format!("{after}|"))),
        "swamp's own README read is shown as the last read: {lr:?}"
    );
}

/// REV2-2 (audit/v080-g7b). docs/usage.md says of a weight file swamp
/// does not read: "the row says the count is \"not read yet\" and why".
/// The reason is only a `params_deferred` field; the text report and the
/// TUI render a row's `about`, `last_read`, `regeneration`, `revisions`,
/// `hub` and `facts`, never `fields`, so the row shows no count and no
/// reason (seen on the real binary: `whisper · float32`, nothing else).
/// Tempting wrong patch: storing the reason as a field and calling it
/// shown because the JSON evidence carries it.
#[test]
fn rev2_a_deferred_count_says_not_read_yet_on_the_row() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    one_repo(
        &root,
        "models--org--fresh",
        &[("model.safetensors", st_header(&[("w", "F32", &[10])]))],
    );
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    let rows = model_rows(&root, &units, 2_000_000_000);
    let row = rows.iter().find(|r| r.name == "org/fresh").unwrap();
    let shown = format!("{:?} {:?}", row.about, row.facts);
    assert!(
        shown.contains("not read yet"),
        "the rendered parts of the row do not say the count is not read yet: {shown}"
    );
}

/// REV2-3 (audit/v080-g7b). `read_would_move_atime` compares the access
/// time with `max(mtime, ctime)`, but APFS moves it on a read only when it
/// is not newer than the *mtime* (checked on this Mac: a file with
/// mtime < atime < ctime keeps its atime through a read). A cache copied
/// with `cp -p`, `rsync -a`, Migration Assistant or a restore has a ctime
/// newer than every atime; a later model load does not move the atime
/// either, so such a model stays "not read yet" for good. Tempting wrong
/// patch: adding ctime as a precaution without checking what APFS does.
#[test]
fn rev2_a_copied_cache_with_a_newer_ctime_is_still_read() {
    use std::os::unix::fs::MetadataExt;
    if !cfg!(target_os = "macos") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let r = one_repo(
        &root,
        "models--org--copied",
        &[("model.safetensors", st_header(&[("w", "F32", &[10])]))],
    );
    let w = r.join("blobs/b0");
    // As `rsync -a` leaves it: mtime and atime from the source (read
    // after its last write), ctime now.
    let m = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let a = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_100_000);
    fs::File::options()
        .write(true)
        .open(&w)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(m).set_accessed(a))
        .unwrap();
    let meta = fs::symlink_metadata(&w).unwrap();
    assert!(meta.ctime() > meta.atime() && meta.atime() > meta.mtime());
    let (units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &CardCache::default());
    assert_eq!(
        fs::symlink_metadata(&w).unwrap().atime(),
        1_700_100_000,
        "the read moved the access time"
    );
    assert_eq!(
        field(unit(&units, "models--org--copied"), "params"),
        Some("10"),
        "a weight file APFS would not touch on a read is deferred anyway"
    );
}

/// REV2-4 (audit/v080-g7b, seen on the real binary). An unchanged hub
/// store's units are replayed from the last pass with their evidence,
/// including the Hub line ("not yet fetched ..."). `hub_api::enrich`
/// then *amends* the unit, adding the new answer beside the old line;
/// `model_row_of` takes the first, so the row keeps saying "not yet
/// fetched" after every later fetch (the real store's JSON held both
/// lines for both repos after an enriching observe). Tempting wrong
/// patch: `NestedUnitBuilder::amend(..).evidence(..)` without removing
/// the unit's earlier `model-hub-api` evidence.
#[test]
fn rev2_a_replayed_unit_shows_the_new_hub_answer_not_the_old_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    one_repo(
        &root,
        "models--org--m",
        &[("config.json", br#"{"model_type":"bert"}"#.to_vec())],
    );
    let cards = CardCache::default();
    let (mut units, _) = identify(BuildStoreKind::HuggingFaceHub, &root, &cards);
    // A pass that may not fetch (the TUI's refresh, or `--no-enrich`).
    crate::hub_api::enrich_with(&mut units, &cards, 1_000, true, 0, &|_, _| {
        Err("unreachable".into())
    });
    // Next pass: the store is unchanged, so the same units come back
    // (replay) and this pass fetches.
    crate::hub_api::enrich_with(&mut units, &cards, 2_000, true, 16, &|_, _| {
        Err("no answer (offline or blocked)".into())
    });
    let rows = model_rows(&root, &units, 3_000);
    let hub = rows[0].hub.clone().unwrap_or_default();
    assert!(
        hub.contains("did not answer"),
        "the row still shows the earlier Hub line: {hub}"
    );
}

/// Restores the original ADV-13 coverage for volumes where a read does
/// move the access time (network volumes, strictatime): the card cache
/// keeps the time from before swamp's read, and a re-parse of the same
/// revision (a new snapshot file) still shows the user's time, not
/// swamp's. Simulated by setting the access time after the first pass.
/// Tempting wrong patch: dropping the before/after correction because
/// APFS seldom needs it.
#[test]
fn where_a_read_moves_the_access_time_the_users_time_survives_a_reparse() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("hub");
    let r = one_repo(
        &root,
        "models--org--nfs",
        &[("model.safetensors", st_header(&[("w", "F32", &[3])]))],
    );
    let w = r.join("blobs/b0");
    used(&w);
    let user = {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(&w).unwrap().atime()
    };
    let cards = CardCache::default();
    let _ = identify(BuildStoreKind::HuggingFaceHub, &root, &cards);
    // The volume moved the access time on swamp's read.
    let key = hub_card_key(&root, "models--org--nfs", REV1);
    let mut e = cards.peek(&key).unwrap();
    let swamp_read = user + 1000;
    e.fields
        .insert("read_atime_after".into(), swamp_read.to_string());
    let mut stored = cards.retained(&|_| false);
    stored.insert(key, e);
    fs::File::options()
        .write(true)
        .open(&w)
        .unwrap()
        .set_times(fs::FileTimes::new().set_accessed(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(swamp_read as u64),
        ))
        .unwrap();
    let (units, _) = identify(
        BuildStoreKind::HuggingFaceHub,
        &root,
        &CardCache::from_entries(stored, 64),
    );
    let lr = ev(unit(&units, "models--org--nfs"), LAST_READ_EVIDENCE)[0];
    assert!(lr.starts_with(&format!("{user}|")), "{lr}");
}
