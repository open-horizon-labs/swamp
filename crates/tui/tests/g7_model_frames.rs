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
    let at = a.rows().iter().position(|r| r.expandable).unwrap();
    a.selected = at;
    handle_key(a, KeyCode::Right);
    let at = a
        .rows()
        .iter()
        .position(|r| r.label.starts_with("mkrausio/EmoWhisper"))
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
        assert!(s.contains("card: Whisper fine-tuned"), "{w}x{h}:\n{s}");
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
