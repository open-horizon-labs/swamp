//! v0.8.0 G7: a Hugging Face repo and an Ollama model as the External and
//! Reclaim views draw them, at 80 and 120 columns. The units come from the
//! real adapter run over a temp fixture; the TUI only reads them.
//!
//! Each test names the tempting wrong patch it fails.

use std::os::unix::fs::symlink;
use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use swamp_core::artifact::NestedArtifact;
use swamp_core::build_adapters::{
    BuildAdapter, BuildContainer, BuildCtx, ContainerCache, FoldedDir, FoldedIndex,
    model_cards::CardCache, model_stores,
};
use swamp_core::drilldown::{ChildKind, ChildMeasure, UnitChild};
use swamp_core::external::ExternalUnit;
use swamp_core::last_used::LastUsed;
use swamp_core::locations::{BuildStoreKind, Provenance, StorageCategory};
use swamp_tui::app::{App, ViewKind};
use swamp_tui::{handle_key, ui};

const REV: &str = "e613edc6dfef73525a11faa84f1f94f10e5a1744";

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    hub: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let hub = root.join("hub");
    let repo = hub.join("models--mkrausio--EmoWhisper-AnS-Small-v0.1");
    for d in ["blobs", "refs"] {
        std::fs::create_dir_all(repo.join(d)).unwrap();
    }
    std::fs::create_dir_all(repo.join("snapshots").join(REV)).unwrap();
    std::fs::write(repo.join("refs/main"), REV).unwrap();
    std::fs::write(
        repo.join("blobs/c"),
        r#"{"model_type":"whisper","architectures":["WhisperForConditionalGeneration"],"torch_dtype":"float32"}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("blobs/r"),
        "---\npipeline_tag: automatic-speech-recognition\nlicense: apache-2.0\n---\nWhisper fine-tuned to tag emotion in speech.\n",
    )
    .unwrap();
    let json = r#"{"w":{"dtype":"F32","shape":[1000,241],"data_offsets":[0,0]}}"#;
    let mut st = (json.len() as u64).to_le_bytes().to_vec();
    st.extend(json.as_bytes());
    st.extend(vec![0u8; 8000]);
    std::fs::write(repo.join("blobs/w"), st).unwrap();
    // Read by a program since it was written, as a loaded model is.
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(repo.join("blobs/w")).unwrap();
        let at = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs((m.mtime().max(m.ctime()) + 60) as u64);
        std::fs::File::options()
            .write(true)
            .open(repo.join("blobs/w"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_accessed(at))
            .unwrap();
    }
    for (n, b) in [
        ("config.json", "c"),
        ("README.md", "r"),
        ("model.safetensors", "w"),
    ] {
        symlink(
            format!("../../blobs/{b}"),
            repo.join("snapshots").join(REV).join(n),
        )
        .unwrap();
    }
    Fx {
        _tmp: tmp,
        root,
        hub,
    }
}

fn interiors(f: &Fx) -> Vec<NestedArtifact> {
    let repo = f.hub.join("models--mkrausio--EmoWhisper-AnS-Small-v0.1");
    let idx = FoldedIndex::from_dirs(vec![
        FoldedDir {
            path: repo.clone(),
            allocated_total: 40_960,
            mtime_max: 5,
            complete: true,
        },
        FoldedDir {
            path: f.hub.clone(),
            allocated_total: 40_960,
            mtime_max: 5,
            complete: true,
        },
    ]);
    let none = swamp_core::fs_events::EventCoverage::untrusted();
    let cache = ContainerCache::disabled();
    let cards = CardCache::default();
    let c = BuildContainer::shared_store_of(
        "model-stores",
        f.hub.clone(),
        BuildStoreKind::HuggingFaceHub,
    );
    let ctx = BuildCtx::new(1_000, &idx, &none, &cache).with_cards(&cards);
    let mut units = model_stores::Adapter.identify(&c, &ctx);
    // What a scheduled observe with Hub facts off adds.
    swamp_core::hub_api::enrich(&mut units, &cards, 1_000, false, true);
    units
}

fn hub_unit(f: &Fx) -> ExternalUnit {
    ExternalUnit {
        detector_id: "fixture".into(),
        detector_name: "Hugging Face cache".into(),
        category: StorageCategory::Models,
        provenance: Provenance::BuiltinConvention,
        path: f.hub.clone(),
        bytes: 40_960,
        mtime_max: 0,
        hardlinked: false,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        consumers: Vec::new(),
        note: None,
        evidence: Vec::new(),
        bytes_counted_elsewhere: 0,
        overlap_count: 0,
        last_used: LastUsed::default(),
        children: vec![UnitChild {
            kind: ChildKind::Entry,
            name: "models--mkrausio--EmoWhisper-AnS-Small-v0.1".into(),
            bytes: Some(40_960),
            measure: ChildMeasure::Complete,
            mtime_max: 0,
            entries: 0,
            not_measured: 0,
            last_used: LastUsed::default(),
        }],
    }
}

fn app(f: &Fx, view: ViewKind) -> App {
    let mut report = swamp_core::report::Report::empty(f.root.clone());
    report.observed_at = 1_000;
    let mut a = App::new(report, f.root.clone());
    a.set_external_units(vec![hub_unit(f)]);
    a.set_store_interiors(interiors(f));
    a.views_seen = true;
    a.set_view(view);
    a
}

fn frame(a: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|fr| ui::draw(fr, a)).unwrap();
    t.backend().to_string()
}

fn open_repo_row(a: &mut App) {
    let hub_key = format!("store-open:{}", a.root.join("hub").display());
    let at = a
        .rows()
        .iter()
        .position(|r| r.expansion_key.as_deref() == Some(hub_key.as_str()))
        .expect("the Hugging Face hub row is identified by its path");
    a.selected = at;
    handle_key(a, KeyCode::Right);
    let at = a
        .rows()
        .iter()
        .position(|r| {
            r.depth == 1
                && r.label
                    .starts_with("folder: mkrausio/EmoWhisper-AnS-Small-v0.1")
        })
        .unwrap_or_else(|| {
            panic!(
                "no repo row: {:?}",
                a.rows().iter().map(|r| r.label.clone()).collect::<Vec<_>>()
            )
        });
    a.selected = at;
}

/// Tempting wrong patch: showing the hub cache as one opaque `models`
/// row (what 0.7 did), or the repo folder under its on-disk name only.
/// The repo row says what the model is at both widths, and the detail
/// pane gives the card, revision, last read and how it comes back.
#[test]
fn external_repo_row_and_detail_say_what_the_model_is_at_80_and_120() {
    let f = fx();
    let mut a = app(&f, ViewKind::External);
    open_repo_row(&mut a);
    let row = &a.rows()[a.selected];
    assert!(
        row.label
            .contains("automatic-speech-recognition · whisper · 241.0K params · float32"),
        "{}",
        row.label
    );
    let detail = row.detail_lines.join("\n");
    assert!(
        detail.contains("card: Whisper fine-tuned to tag emotion in speech."),
        "{detail}"
    );
    assert!(detail.contains("regeneration: downloaded again from huggingface.co (mkrausio/EmoWhisper-AnS-Small-v0.1@e613edc6) when needed"), "{detail}");
    assert!(
        detail.contains("hf_enrich"),
        "the off line says how to turn it on: {detail}"
    );
    for (w, h) in [(80, 24), (120, 30)] {
        let s = frame(&a, w, h);
        assert!(s.contains("mkr"), "{w}x{h}:\n{s}");
        assert!(
            s.contains("what it is: automatic-speech-recognition · whisper"),
            "{w}x{h}:\n{s}"
        );
        assert!(
            s.contains("model 41KB attributed total") && s.contains("folder 41KB"),
            "{w}x{h}:
{s}"
        );
    }
}

/// Tempting wrong patch: the Reclaim child row keeps only the folder
/// name. Its text carries the "what it is" line too.
#[test]
fn reclaim_child_text_carries_the_card_line() {
    let f = fx();
    let ints = interiors(&f);
    let units = vec![hub_unit(&f)];
    let view = swamp_core::reclaim::build(&swamp_core::reclaim::ReclaimInput {
        units: &units,
        interiors: &ints,
        unowned: &[],
        manager_facts: &Default::default(),
        declared_roots: &[],
        explicit_scope: false,
        projects: 0,
        observed_at: 1_000,
    });
    let row = &view.rows[0];
    assert!(
        row.children[0]
            .text
            .contains("· automatic-speech-recognition"),
        "{row:#?}"
    );
    assert_eq!(row.models.len(), 1);
    assert!(
        row.regeneration.words.contains("downloaded again")
            && !row.regeneration.words.contains("cannot be regenerated"),
        "{:?}",
        row.regeneration
    );
    let text = swamp_core::reclaim::render_text(&view);
    assert!(text.contains("models (1), largest first"), "{text}");
}

/// Tempting wrong patch: reading the card when the TUI draws. Drawing
/// does no file read and starts no process.
#[test]
fn drawing_reads_no_file_and_starts_no_process() {
    let f = fx();
    let mut a = app(&f, ViewKind::External);
    let (_, work) = swamp_core::work_counters::measured(|| {
        open_repo_row(&mut a);
        let _ = frame(&a, 120, 30);
        let _ = frame(&a, 80, 24);
    });
    assert_eq!(work.header_bytes_read, 0);
    assert_eq!(work.subprocess_spawns, 0);
}

/// Audit item 4. Tempting wrong patch: the confirm and the detail pane
/// give the repo folder's size and say nothing of the shared blob it
/// leaves behind; Ollama tags only in JSON. The hub repo's confirm says
/// what stays, and an Ollama tag is a row of its own in Reclaim and
/// External with what it is and that its layers stay.
#[test]
fn what_a_move_leaves_behind_is_on_the_confirm_and_ollama_tags_are_rows() {
    let f = fx();
    // A repo whose weights live in the hub's shared blobs/.
    let repo = f.hub.join("models--org--shared");
    std::fs::create_dir_all(repo.join("blobs")).unwrap();
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::create_dir_all(f.hub.join("blobs/ab")).unwrap();
    std::fs::write(f.hub.join("blobs/ab/big"), vec![1u8; 300_000]).unwrap();
    symlink("../../blobs/ab/big", repo.join("blobs/w")).unwrap();
    std::fs::write(repo.join("refs/main"), REV).unwrap();
    let mut dirs = vec![
        FoldedDir {
            path: repo.clone(),
            allocated_total: 4096,
            mtime_max: 5,
            complete: true,
        },
        FoldedDir {
            path: f.hub.join("blobs"),
            allocated_total: 307_200,
            mtime_max: 5,
            complete: true,
        },
        FoldedDir {
            path: f.hub.join("models--mkrausio--EmoWhisper-AnS-Small-v0.1"),
            allocated_total: 40_960,
            mtime_max: 5,
            complete: true,
        },
    ];
    dirs.push(FoldedDir {
        path: f.hub.clone(),
        allocated_total: 352_256,
        mtime_max: 5,
        complete: true,
    });
    let idx = FoldedIndex::from_dirs(dirs);
    let none = swamp_core::fs_events::EventCoverage::untrusted();
    let cache = ContainerCache::disabled();
    let cards = CardCache::default();
    let c = BuildContainer::shared_store_of(
        "model-stores",
        f.hub.clone(),
        BuildStoreKind::HuggingFaceHub,
    );
    let mut ints = model_stores::Adapter.identify(
        &c,
        &BuildCtx::new(1_000, &idx, &none, &cache).with_cards(&cards),
    );
    // An Ollama store with one tag.
    let o = f.root.join("ollama");
    let lib = o.join("manifests/registry.ollama.ai/library/qwen3");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(o.join("blobs")).unwrap();
    let a = format!("sha256-{}", "a".repeat(64));
    let cfg = format!("sha256-{}", "c".repeat(64));
    std::fs::write(o.join("blobs").join(&a), vec![3u8; 50_000]).unwrap();
    std::fs::write(o.join("blobs").join(&cfg), r#"{"model_format":"gguf","model_family":"qwen3","model_type":"751.63M","file_type":"Q4_K_M"}"#).unwrap();
    std::fs::write(lib.join("0.6b"), format!(r#"{{"config":{{"mediaType":"x","digest":"sha256:{}","size":1}},"layers":[{{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:{}","size":50000}}]}}"#, "c".repeat(64), "a".repeat(64))).unwrap();
    let oidx = FoldedIndex::from_dirs(vec![
        FoldedDir {
            path: o.join("blobs"),
            allocated_total: 61_440,
            mtime_max: 5,
            complete: true,
        },
        FoldedDir {
            path: o.clone(),
            allocated_total: 65_536,
            mtime_max: 5,
            complete: true,
        },
    ]);
    let oc =
        BuildContainer::shared_store_of("model-stores", o.clone(), BuildStoreKind::OllamaModels);
    ints.extend(model_stores::Adapter.identify(
        &oc,
        &BuildCtx::new(1_000, &oidx, &none, &cache).with_cards(&cards),
    ));
    let mut hub = hub_unit(&f);
    hub.children.push(UnitChild {
        kind: ChildKind::Entry,
        name: "models--org--shared".into(),
        bytes: Some(4096),
        measure: ChildMeasure::Complete,
        mtime_max: 0,
        entries: 0,
        not_measured: 0,
        last_used: LastUsed::default(),
    });
    let mut ou = hub_unit(&f);
    ou.path = o.clone();
    ou.detector_name = "Ollama".into();
    ou.bytes = 65_536;
    ou.children = vec![];
    let units = vec![hub, ou];
    let view = swamp_core::reclaim::build(&swamp_core::reclaim::ReclaimInput {
        units: &units,
        interiors: &ints,
        unowned: &[],
        manager_facts: &Default::default(),
        declared_roots: &[],
        explicit_scope: false,
        projects: 0,
        observed_at: 1_000,
    });
    let t = swamp_core::reclaim_trash::find_target(&view, &repo).unwrap();
    assert!(
        t.notes
            .iter()
            .any(|n| n.contains("moving this folder frees about")
                && n.contains("stays in the hub's shared blobs/")),
        "{:?}",
        t.notes
    );
    let tag = swamp_core::reclaim_trash::find_target(&view, &lib.join("0.6b")).unwrap();
    assert!(
        tag.notes
            .iter()
            .any(|n| n.contains("stay in blobs/") && n.contains("`ollama rm qwen3:0.6b`")),
        "{:?}",
        tag.notes
    );
    // The tag is a row in Reclaim and in External, with what it is.
    let mut report = swamp_core::report::Report::empty(f.root.clone());
    report.observed_at = 1_000;
    for view_kind in [ViewKind::Reclaim, ViewKind::External] {
        let mut a = App::new(report.clone(), f.root.clone());
        a.set_external_units(units.clone());
        a.set_store_interiors(ints.clone());
        a.views_seen = true;
        a.set_view(view_kind);
        let store_path = f.hub.display().to_string();
        let store_at = a
            .rows()
            .iter()
            .position(|r| r.expandable && r.label.contains(&store_path))
            .unwrap_or_else(|| panic!("{view_kind:?}: no hub row: {:?}", a.rows()));
        a.selected = store_at;
        handle_key(&mut a, KeyCode::Right);
        let repo_at = a
            .rows()
            .iter()
            .position(|r| {
                r.label.starts_with("folder:")
                    && r.label.contains("shared")
                    && r.signals.iter().any(|s| s.starts_with("model "))
            })
            .unwrap_or_else(|| {
                panic!(
                    "{view_kind:?}: no labeled repo-folder row: {:?}",
                    a.rows().iter().map(|r| r.label.clone()).collect::<Vec<_>>()
                )
            });
        a.selected = repo_at;
        let repo_row = &a.rows()[repo_at];
        let model = model_stores::model_rows(&f.hub, &ints, 1_000)
            .into_iter()
            .find(|m| m.name == "org/shared")
            .unwrap();
        assert_eq!(
            repo_row.bytes, 4096,
            "Size stays the repo folder allocation"
        );
        assert_ne!(
            repo_row.bytes, model.bytes,
            "model attribution is not the path size"
        );
        let storage_signal = repo_row
            .signals
            .iter()
            .find(|s| s.starts_with("model "))
            .unwrap();
        assert!(storage_signal.contains(&swamp_tui::model::human_bytes(model.bytes)));
        assert!(storage_signal.contains("incl. shared blobs"));
        assert!(storage_signal.contains(&swamp_tui::model::human_bytes(repo_row.bytes)));
        let detail = repo_row.detail_lines.join("\n");
        assert!(detail.contains("not additive"), "{detail}");
        assert!(
            detail.contains("moving this folder frees about")
                && detail.contains("stays in the hub's shared blobs/"),
            "{detail}"
        );
        let narrow = frame(&a, 80, 24);
        let wide = frame(&a, 200, 60);
        assert!(
            narrow.contains("folder:"),
            "{view_kind:?} narrow frame:\n{narrow}"
        );
        assert!(
            narrow.contains(storage_signal.split(" · ").next().unwrap()),
            "{view_kind:?} narrow frame omitted model attribution:\n{narrow}"
        );
        assert!(
            wide.contains(storage_signal),
            "{view_kind:?} wide frame:\n{wide}"
        );
        for _ in 0..4 {
            let Some(at) = a
                .rows()
                .iter()
                .position(|r| r.expandable && r.collapsed_children.is_some())
            else {
                break;
            };
            a.selected = at;
            handle_key(&mut a, KeyCode::Right);
        }
        let row = a
            .rows()
            .into_iter()
            .find(|r| {
                r.label.contains("qwen3:0.6b")
                    && r.label.contains("qwen3 · 751.63M params · Q4_K_M")
            })
            .unwrap_or_else(|| {
                panic!(
                    "{view_kind:?}: no tag row: {:?}",
                    a.rows().iter().map(|r| r.label.clone()).collect::<Vec<_>>()
                )
            });
        assert!(
            row.detail_lines
                .iter()
                .any(|l| l.contains("stay in blobs/")),
            "{:?}",
            row.detail_lines
        );
        assert!(row.unit.is_some(), "the manifest is markable");
    }
}
