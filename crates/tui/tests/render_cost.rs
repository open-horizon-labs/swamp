//! Manual read-only rendering replay against a copied real observation store.
use std::path::PathBuf;
use std::time::Instant;

use ratatui::{Terminal, backend::TestBackend};
use swamp_tui::{
    app::{App, ViewKind},
    ui,
};

#[test]
#[ignore = "manual real-store render measurement; set SWAMP_RENDER_READONLY_STORE"]
fn stored_views_render_cost() {
    let store = PathBuf::from(
        std::env::var_os("SWAMP_RENDER_READONLY_STORE").expect("copied store required"),
    );
    let scope = swamp_core::scope::load_last_effective_scope(&store).unwrap();
    let snapshot = swamp_core::report::report_scope_from_store(&scope, &store).unwrap();
    let mut app = App::new_multi_root(snapshot.report, scope.scan_paths());
    app.set_external_units(snapshot.external_units);
    app.set_store_interiors(snapshot.store_interiors);
    app.set_agent_units(snapshot.agent_units);
    app.set_manager_facts(snapshot.manager_facts);
    app.set_ledger(swamp_core::volume_ledger::read_reading(&store));
    app.scope = Some(scope);
    app.selected_project = app
        .report
        .projects
        .iter()
        .find(|p| p.name.contains("swamp"))
        .map(|p| p.name.clone());
    app.clear_filter();
    app.live_age = true;
    // Optional artifact capture uses the same copied store as the replay.
    if let Some(dir) = std::env::var_os("SWAMP_CLARITY_CAPTURE") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for view in ViewKind::ALL {
            app.set_view(view);
            for (name, width, height) in [("compact", 80, 24), ("wide", 200, 60)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| ui::draw(f, &app)).unwrap();
                let buffer = terminal.backend().buffer();
                let contents = (0..height)
                    .map(|y| {
                        (0..width)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                std::fs::write(dir.join(format!("{}-{name}.txt", view.label())), contents).unwrap();
            }
        }
    }
    for view in [
        ViewKind::Projects,
        ViewKind::Tree,
        ViewKind::Agents,
        ViewKind::Reclaim,
    ] {
        app.set_view(view);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
        let mut samples = Vec::new();
        let (_, work) = swamp_core::work_counters::measured(|| {
            for _ in 0..25 {
                let start = Instant::now();
                terminal.draw(|f| ui::draw(f, &app)).unwrap();
                samples.push(start.elapsed().as_micros());
            }
        });
        samples.sort_unstable();
        eprintln!(
            "view={view:?} rows={} median_us={} p95_us={} dirs={} stats={} spawns={}",
            app.rows().len(),
            samples[12],
            samples[23],
            work.dirs_listed,
            work.files_statted,
            work.subprocess_spawns
        );
        assert_eq!(work.dirs_listed, 0, "rendering cannot walk");
        assert_eq!(work.files_statted, 0, "rendering cannot stat");
        assert_eq!(work.subprocess_spawns, 0, "rendering cannot launch probes");
    }
}
