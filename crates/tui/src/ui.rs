//! ratatui rendering. Diffstat-ledger world: box-drawing rail, reverse
//! video selection, yellow `✗` for marked rows, no other color. See
//! DESIGN.md.

use crate::app::App;
use crate::model::{
    diverging_bar, human_bytes, human_signed_bytes, is_noise, max_abs_growth, net_change,
    pad_display, spark_deltas, truncate_middle,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Sparkline, SparklineBar},
};

/// Growth is the bad news in a disk tool: red when bytes arrive, green
/// when they leave.
const GROW: Color = Color::Red;
const SHRINK: Color = Color::Green;
/// The selected row: a dark background and bold, never reverse video,
/// so the growth colours stay readable on the line you are looking at.
const SELECTED_BG: Color = Color::Indexed(236);

/// Width of every history sparkline, header and rows alike.
const SPARK_WIDTH: u16 = 12;

/// Draws a byte series as a timeline of movement with ratatui's
/// `Sparkline`: each bar is one bucket's change, its height the size of
/// the change relative to the row's largest, red for bytes arriving and
/// green for bytes leaving. A bucket where nothing moved is blank; a
/// bucket before the first observation is a dim `·`.
fn draw_spark(frame: &mut Frame, series: &[Option<u64>], area: Rect, selected: bool) {
    let d = spark_deltas(series, area.width as usize);
    let max = d
        .iter()
        .filter_map(|v| v.map(i64::unsigned_abs))
        .max()
        .unwrap_or(0);
    let bg = |st: Style| if selected { st.bg(SELECTED_BG) } else { st };
    let bars: Vec<SparklineBar> = d
        .iter()
        .map(|v| match v {
            None => SparklineBar::from(None::<u64>),
            Some(0) => SparklineBar::from(Some(0u64)),
            Some(x) => SparklineBar::from(Some(x.unsigned_abs())).style(Some(bg(
                Style::default().fg(if *x > 0 { GROW } else { SHRINK })
            ))),
        })
        .collect();
    frame.render_widget(
        Sparkline::default()
            .data(bars)
            .max(max.max(1))
            .style(bg(Style::default()))
            .absent_value_symbol("·")
            .absent_value_style(bg(Style::default().fg(Color::DarkGray))),
        area,
    );
}

/// The growth window the header reports. When the asked-for window is
/// longer than the observations the store holds, the effective window is
/// the history itself, and the label says so instead of implying a week
/// of growth from four hours of data.
fn since_label(app: &App) -> Option<String> {
    let asked = crate::filter::growth_window_secs(&app.filter)?;
    Some(match app.history_secs {
        Some(hist) if hist < asked => format!(
            "{} (asked {}; history is {})",
            human_duration(hist),
            human_duration(asked),
            human_duration(hist)
        ),
        _ => human_duration(asked),
    })
}

fn human_duration(secs: u64) -> String {
    if secs.is_multiple_of(604_800) {
        format!("{}w", secs / 604_800)
    } else if secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else {
        format!("{}m", (secs / 60).max(1))
    }
}

fn header_line(app: &App, width: usize) -> String {
    let stale = app.report.unowned.iter().any(|u| {
        u.measurement
            .is_some_and(|m| m.unique_needs_reconciliation())
    }) || app
        .report
        .projects
        .iter()
        .flat_map(|p| &p.worktrees)
        .flat_map(|w| &w.artifacts)
        .any(|a| a.dedup_stale);
    let projects = app.report.projects.len();
    let attributed: u64 = app
        .report
        .projects
        .iter()
        .flat_map(|p| &p.worktrees)
        .flat_map(|w| &w.artifacts)
        .map(|a| a.bytes)
        .sum();
    let docker_unowned = crate::model::docker_unowned_bytes(&app.report);
    let unowned: u64 = app
        .report
        .unowned
        .iter()
        .map(|u| u.bytes)
        .sum::<u64>()
        .saturating_sub(docker_unowned);
    let obs = if app.observing.is_some() {
        // Live counters from the walk thread; percent against the last
        // observation's walked total (an incremental walk stops early, so
        // the percent is a floor, never a promise).
        let (bytes, dirs, _) = swamp_core::walk::progress::snapshot();
        let total = app.report.reconciliation.walked_total;
        // The percentage compares against the last observation's total,
        // so it is an estimate; an incremental walk visits a fraction and
        // a grown tree can exceed it. Show it only while it means
        // something.
        let pct = match total {
            0 => String::new(),
            t if bytes <= t => format!(" · {}%", bytes * 100 / t),
            _ => String::new(),
        };
        format!("observing… {} · {dirs} dirs{pct}", human_bytes(bytes))
    } else {
        format!(
            "observed {}{}",
            app.observed_label,
            if !app.watches.is_empty() {
                " · live"
            } else {
                ""
            }
        )
    };
    let since = since_label(app)
        .map(|s| format!(" · since {s}"))
        .unwrap_or_default();
    // Clauses in priority order; the renderer drops trailing clauses that
    // do not fit the terminal width rather than truncating mid-word.
    let clauses = vec![
        if stale
            || app
                .report
                .reconciliation
                .unique_estimate
                .as_ref()
                .is_some_and(|u| u.needs_reconciliation)
        {
            format!("unique totals not recomputed · {}", app.root.display())
        } else {
            app.root.display().to_string()
        },
        format!("{obs}{since}"),
        app.report
            .reconciliation
            .unique_estimate
            .as_ref()
            .map(|u| {
                format!(
                    "{} unique{}",
                    human_bytes(u.bytes),
                    if u.needs_reconciliation {
                        " (needs reconciliation)"
                    } else {
                        " (reconciled)"
                    }
                )
            })
            .unwrap_or_default(),
        format!("{projects} projects"),
        format!("{} attributed", human_bytes(attributed)),
        format!("{} unowned", human_bytes(unowned)),
        format!("docker {} unowned", human_bytes(docker_unowned)),
        app.scope_note.clone().unwrap_or_default(),
    ];
    fit_clauses(&clauses, width)
}

/// Joins clauses with " · " while the result fits in `width`; always keeps
/// the first clause, truncated to terminal cells if necessary.
pub fn fit_clauses(clauses: &[String], width: usize) -> String {
    let mut out = String::new();
    for (i, c) in clauses.iter().filter(|c| !c.is_empty()).enumerate() {
        let candidate = if i == 0 {
            c.clone()
        } else {
            format!("{out} · {c}")
        };
        if i > 0 && width > 0 && crate::model::display_width(&candidate) > width {
            break;
        }
        out = candidate;
    }
    truncate_middle(&out, width)
}

fn footer_line() -> &'static str {
    "↑↓ move  →/← in/out  Enter open/confirm  Space mark  A mark all  ⌫ delete  / filter  v view  g/s/n/t/a sort  r reverse  ? help  q quit"
}

pub fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // filter line
            Constraint::Min(1),    // body
            Constraint::Length(if app.operation.is_some() {
                3
            } else if app.confirm_open {
                1
            } else {
                0
            }),
            Constraint::Length(1), // footer
        ])
        .split(size);

    draw_header(frame, app, chunks[0]);

    draw_filter_line(frame, app, chunks[1]);
    draw_body(frame, app, chunks[2]);

    if let Some(op) = &app.operation {
        let cancelling = op.cancel.load(std::sync::atomic::Ordering::SeqCst);
        let ratio = if op.total == 0 {
            0.0
        } else {
            (op.completed as f64 / op.total as f64).min(1.0)
        };
        let label = if cancelling {
            "Cancelling after current group"
        } else {
            op.label
        };
        let count = if op.total == 0 {
            format!("{} checked", op.completed)
        } else {
            format!("{}/{} groups", op.completed, op.total)
        };
        let area = chunks[3];
        frame.render_widget(
            ratatui::widgets::Gauge::default()
                .ratio(ratio)
                .label(format!(
                    "{label} · {count} · {}s",
                    op.started.elapsed().as_secs()
                ))
                .gauge_style(Style::default().fg(Color::Yellow)),
            Rect { height: 1, ..area },
        );
        frame.render_widget(
            Paragraph::new(format!(
                "{} successful · {} refused · {}",
                op.succeeded,
                op.failed,
                op.current.display()
            )),
            Rect {
                y: area.y + 1,
                height: area.height.saturating_sub(1).min(1),
                ..area
            },
        );
        frame.render_widget(
            Paragraph::new(if op.label == "Reviewing" {
                "Review only; no files are changed. Cancellation preserves previous marks."
            } else {
                "Filesystem cleanup moves to Trash; allocated bytes are not freed space."
            }),
            Rect {
                y: area.y + 2,
                height: area.height.saturating_sub(2).min(1),
                ..area
            },
        );
    } else if app.confirm_open {
        frame.render_widget(
            Paragraph::new(app.confirm_summary()).style(Style::default().fg(Color::Yellow)),
            chunks[3],
        );
    }

    // The footer is the key legend for the state you are actually in.
    let footer_text = if let Some(op) = &app.operation {
        if op.label == "Reviewing" {
            "Esc / Ctrl-C: cancel review after current check".into()
        } else {
            "Esc / Ctrl-C: stop after current group · completed moves remain in Trash".into()
        }
    } else if let Some(msg) = app.refusal_active() {
        msg.to_string()
    } else if app.confirm_open {
        "Enter yes · Esc no".to_string()
    } else if app.picker.is_some() {
        "↑↓ field · ←→ value · Space grew/shrank · type to narrow project · Enter apply · Esc cancel · e edit as text · 0 clear".to_string()
    } else if app.editing_filter {
        "Tab complete · Enter apply · Esc cancel".to_string()
    } else if let Some(r) = &app.last_result {
        r.clone()
    } else {
        footer_line().to_string()
    };
    let footer_style = if app.refusal_active().is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };
    frame.render_widget(Paragraph::new(footer_text).style(footer_style), chunks[4]);

    if app.help_open {
        draw_help(frame, size);
    }
    if let Some(p) = &app.picker {
        draw_picker(frame, app, p, size);
    }
    if let Some(lines) = &app.cargo_inspection {
        let popup = Rect {
            x: size.x + 1,
            y: size.y + 1,
            width: size.width.saturating_sub(2),
            height: size.height.saturating_sub(2),
        };
        frame.render_widget(Clear, popup);
        let visible: Vec<Line> = lines
            .iter()
            .skip(app.cargo_inspection_scroll as usize)
            .take(popup.height.saturating_sub(2) as usize)
            .map(|s| Line::from(s.as_str()))
            .collect();
        frame.render_widget(
            Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(
                " Cargo dependency inspection · ↑↓ scroll · Esc close · no cleanup action ",
            )),
            popup,
        );
    }
}

fn draw_picker(frame: &mut Frame, app: &App, p: &crate::picker::Picker, area: Rect) {
    let w = area.width.min(78);
    let h = area.height.min(18);
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, popup);
    let composed = p.compose();
    let count = app
        .match_count_for(&composed)
        .map(|n| format!("{n} row{}", if n == 1 { "" } else { "s" }))
        .unwrap_or_else(|| "—".into());
    let mut lines: Vec<Line> = Vec::new();
    for (name, value, selected) in p.lines() {
        let marker = if selected { "▸ " } else { "  " };
        let l = Line::from(format!("{marker}{name:<15}{value}"));
        lines.push(if selected {
            l.style(Style::default().add_modifier(ratatui::style::Modifier::REVERSED))
        } else {
            l
        });
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!("  filter: {composed}    → {count}")));
    lines.push(Line::from("  ↑↓ field · ←→ value · type to narrow project"));
    lines.push(Line::from(
        "  Enter apply · Esc cancel · e edit as text · 0 clear",
    ));
    let block = Block::default().borders(Borders::ALL).title(" filter ");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Header: facts on the left, the whole root's history on the right as a
/// sparkline with its net change over the window (first observed to last).
fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let total = &app.report.total_series;
    let net = net_change(total);
    let right_width: u16 = match net {
        Some(d) if area.width >= 80 => SPARK_WIDTH + 1 + human_signed_bytes(d).len() as u16,
        _ => 0,
    };
    let left = Rect {
        width: area.width.saturating_sub(right_width + 2),
        ..area
    };
    frame.render_widget(
        Paragraph::new(header_line(app, left.width as usize))
            .style(Style::default().add_modifier(Modifier::DIM)),
        left,
    );
    if let Some(d) = net.filter(|_| right_width > 0) {
        let x = area.x + area.width - right_width;
        draw_spark(
            frame,
            total,
            Rect {
                x,
                y: area.y,
                width: SPARK_WIDTH,
                height: 1,
            },
            false,
        );
        frame.render_widget(
            Paragraph::new(human_signed_bytes(d))
                .style(Style::default().add_modifier(Modifier::DIM)),
            Rect {
                x: x + SPARK_WIDTH + 1,
                y: area.y,
                width: right_width - SPARK_WIDTH - 1,
                height: 1,
            },
        );
    }
}

fn draw_filter_line(frame: &mut Frame, app: &App, area: Rect) {
    let text = if app.editing_filter {
        let hint = if app.completions.is_empty() {
            "(Tab complete · Enter apply · Esc cancel)".to_string()
        } else {
            format!("(Tab: {})", app.completions.join("  "))
        };
        format!("filter › {}▏  {hint}", app.filter_text)
    } else {
        let scope = match (app.view, app.selected_project.as_deref()) {
            (crate::app::ViewKind::Projects, _) => "projects".to_string(),
            (crate::app::ViewKind::Tree, Some(p)) => format!("tree of {p}  (Esc back)"),
            (v, Some(p)) => format!("{} of {p}  (Esc back)", v.label()),
            (v, None) => format!("{}  (Esc back)", v.label()),
        };
        let sort_name = match app.sort {
            crate::model::Sort::Growth => Some("growth"),
            crate::model::Sort::Size => Some("size"),
            crate::model::Sort::Name => Some("name"),
            crate::model::Sort::Type => Some("type"),
            crate::model::Sort::Age => Some("age"),
            crate::model::Sort::None => None,
        };
        let sort = match sort_name {
            Some(n) if app.reverse => format!(" · sort: {n} ↑"),
            Some(n) => format!(" · sort: {n}"),
            None => String::new(),
        };
        let filter = if app.filter_text == "0" {
            "none".to_string()
        } else {
            app.filter_text.clone()
        };
        format!("view: {scope} · filter: {filter}{sort}")
    };
    frame.render_widget(Paragraph::new(text), area);
    if let Some(err) = &app.filter_error {
        // Parse errors show inline in red under the line; with only one
        // row budgeted here we overlay on the same line's tail instead
        // of stealing a row from the body, keeping the one-screen rhythm.
        let msg = format!("  parse error: {err}");
        let x = area.x + (app.filter_text.len() as u16) + 9;
        if x < area.x + area.width {
            let sub = Rect {
                x,
                y: area.y,
                width: area.width.saturating_sub(x - area.x),
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(msg).style(Style::default().fg(Color::Red)),
                sub,
            );
        }
    }
}

/// Orders a row's decision evidence for the detail area (#60): activity
/// (with its timestamp meaning/source/freshness), consumers, current-
/// use, recovery, reclaimability -- the order named in the acceptance
/// criteria, so a terminal too short to show every line clips the tail
/// (the least decision-relevant facts), never the front.
fn ordered_evidence_lines(evidence: &[swamp_core::evidence::Evidence]) -> Vec<String> {
    use swamp_core::evidence::FactKind;
    fn priority(k: FactKind) -> u8 {
        match k {
            FactKind::Activity => 0,
            FactKind::Consumer => 1,
            FactKind::CurrentUse => 2,
            FactKind::Recovery => 3,
            FactKind::Reclaimability => 4,
        }
    }
    let mut sorted: Vec<swamp_core::evidence::Evidence> = evidence.to_vec();
    sorted.sort_by_key(|e| priority(e.kind));
    swamp_core::render::render_evidence_lines(&sorted)
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    if app.filter_has_no_data() {
        frame.render_widget(Paragraph::new("no data yet"), area);
        return;
    }
    let rows = app.rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new("no rows match — / to change the filter, 0 to clear"),
            area,
        );
        return;
    }
    let max_abs = max_abs_growth(rows.iter());
    // Columns scale with the terminal: fixed bytes (10) + growth (10) +
    // bar; the rest is split between the name and the signals so a wide
    // terminal shows whole paths and spelled-out signals instead of the
    // 80-column layout centered in empty space.
    let width = area.width as usize;
    let narrow = width < 120;
    // Half the diverging bar, each side; plus one cell for the axis.
    let cleanup_view = rows.iter().any(|r| r.cleanup_summary.is_some());
    let show_growth = !cleanup_view || width >= 100;
    let half: usize = if width >= 140 && !cleanup_view { 6 } else { 0 };
    // name | bytes(10) | sp | growth(10) | sp | half│half | sp | signals
    let bar_width = if half > 0 { half * 2 + 2 } else { 0 };
    let fixed = if show_growth { 24 + bar_width } else { 13 };
    let flexible = width.saturating_sub(fixed);
    let signals_width = if cleanup_view {
        (flexible / 2).min(64)
    } else if width >= 100 {
        flexible / 3
    } else {
        0
    };
    let name_width = if cleanup_view {
        flexible.saturating_sub(signals_width).min(64)
    } else {
        flexible.saturating_sub(signals_width)
    };
    let signals_width = if cleanup_view {
        flexible.saturating_sub(name_width)
    } else {
        signals_width
    };
    let heading = format!(
        "{}{:>10} {}{}{}",
        pad_display("Name", name_width),
        if cleanup_view { "Size*" } else { "Size" },
        if show_growth {
            format!("{:>10} ", "Change")
        } else {
            String::new()
        },
        pad_display(if half > 0 { "Change bar" } else { "" }, bar_width),
        pad_display(
            if cleanup_view {
                "Cleanup advice / consequence"
            } else {
                "Cleanup / facts"
            },
            signals_width
        )
    );
    let mut lines: Vec<Line> = vec![Line::styled(
        heading,
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if cleanup_view {
        lines.push(Line::raw(
            "* allocated incl. shared links; not additive with report totals. Age = modified",
        ));
    }
    for (i, row) in rows.iter().enumerate() {
        let mut marked = row
            .unit
            .as_ref()
            .is_some_and(|u| app.marked.contains_key(&u.0));
        if let Some(key) = row
            .expansion_key
            .as_deref()
            .filter(|k| crate::model::is_cleanup_selection(&app.report, k))
        {
            let members = crate::model::cleanup_members(&app.report, key);
            marked = !members.is_empty()
                && members
                    .iter()
                    .all(|u| app.marked.contains_key(&u.path.display().to_string()));
        }
        let mark_prefix = if marked { "✗ " } else { "" };
        let track = match row.track {
            Some(t) if !t.label().is_empty() => format!("  [{}]", t.label()),
            _ => String::new(),
        };
        // Badges trail the name so names stay left-aligned and scannable.
        let badge = if row.badges.is_empty() {
            String::new()
        } else {
            format!("  {}", row.badges)
        };
        let raw_name = format!("{}{mark_prefix}{}{badge}{track}", row.rail, row.label);
        let name = truncate_middle(&raw_name, name_width);
        let bytes = format!(
            "{:>10}",
            format!(
                "{}{}",
                human_bytes(row.bytes),
                if row.allocated { "*" } else { "" }
            )
        );
        let growth = format!(
            "{:>10}",
            row.growth
                .map(human_signed_bytes)
                .unwrap_or_else(|| "—".into())
        );
        let (bar_left, axis, bar_right) = diverging_bar(row.growth, max_abs, half);
        let bar_color = match row.growth {
            Some(g) if g > 0 => GROW,
            Some(g) if g < 0 => SHRINK,
            _ => Color::DarkGray,
        };
        // A change too small to act on is dimmed, number and tick alike.
        let bar_style = if is_noise(row.growth) {
            Style::default().fg(bar_color).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(bar_color)
        };
        let hidden = row
            .collapsed_children
            .map(|n| format!("  ▸ {n} more"))
            .unwrap_or_default();
        // DESIGN.md: "80x24 ... signals drop to a single glyph column;
        // uses width up to 200 ... signals spell out."
        // Narrow terminals show the two most decision-relevant signals
        // spelled out (never a glyph code); wide ones show them all.
        let mut signals_text = if let Some(summary) = &row.cleanup_summary {
            // Drop secondary statistics before clipping the decision itself.
            let mut parts: Vec<_> = summary.split(" · ").collect();
            while parts.len() > 1 && parts.join(" · ").chars().count() > signals_width {
                parts.pop();
            }
            parts.join(" · ")
        } else if row.signals.is_empty() {
            String::new()
        } else if narrow {
            pick_signals(&row.signals, 2).join(" · ")
        } else {
            row.signals.join(" · ")
        };
        if row.cleanup_summary.is_none() && signals_text.chars().count() > signals_width {
            // Never overflow the row: prefer the loud signals, then cut.
            signals_text = pick_signals(&row.signals, 3).join(" · ");
            if signals_text.chars().count() > signals_width {
                signals_text = signals_text
                    .chars()
                    .take(signals_width.saturating_sub(1))
                    .collect::<String>()
                    + "…";
            }
        }

        let name_style = if marked {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        let bar = if half == 0 {
            String::new()
        } else if row.growth.is_none_or(|g| g == 0) {
            " ".repeat(bar_width)
        } else {
            format!("{bar_left}{axis}{bar_right} ")
        };
        let spans = vec![
            Span::styled(pad_display(&name, name_width), name_style),
            Span::raw(bytes),
            Span::raw(" "),
            // The signed number and its bar are one diffstat token: same
            // colour, number flush against the bar it measures.
            Span::styled(if show_growth { growth } else { String::new() }, bar_style),
            Span::raw(if show_growth { " " } else { "" }),
            Span::styled(bar, bar_style),
            Span::styled(
                pad_display(
                    &if row.cleanup_summary.is_some() {
                        signals_text
                    } else {
                        format!("{signals_text}{hidden}")
                    },
                    signals_width,
                ),
                Style::default().add_modifier(if row.cleanup_summary.is_some() {
                    Modifier::BOLD
                } else {
                    Modifier::DIM
                }),
            ),
        ];

        let mut line = Line::from(spans);
        if i == app.selected {
            line = line.style(
                Style::default()
                    .bg(SELECTED_BG)
                    .add_modifier(Modifier::BOLD),
            );
        }
        lines.push(line);
    }
    // Keep the selection visible even in projects with hundreds of build groups.
    let selected_evidence_lines: Vec<String> = rows
        .get(app.selected)
        .map(|r| {
            let path = r
                .unit
                .as_ref()
                .map(|u| std::path::Path::new(&u.0))
                .or_else(|| r.worktree.as_ref().map(|w| w.path.as_path()));
            let mut lines = if let Some(path) = path {
                swamp_core::render::render_sharing_lines(
                    app.report.reconciliation.unique_estimate.as_ref(),
                    Some(path),
                )
            } else {
                Vec::new()
            };
            lines.extend(ordered_evidence_lines(&r.evidence));
            lines
        })
        .unwrap_or_default();
    let base_detail = if cleanup_view { 3 } else { 2 };
    // Each evidence line can itself wrap to several physical rows at a
    // narrow width (`Wrap { trim: true }` below), so sizing by logical
    // fact count alone would silently clip real content -- estimate
    // wrapped rows instead. Capped at half the body height: a unit with
    // many facts must not push the row table itself off screen.
    let content_width = (area.width as usize).max(1);
    let wrapped_rows =
        |s: &str| -> u16 { (s.chars().count().max(1)).div_ceil(content_width) as u16 };
    let evidence_rows: u16 = selected_evidence_lines
        .iter()
        .take(6)
        .map(|l| wrapped_rows(l))
        .sum();
    let detail_height = (base_detail + evidence_rows)
        .min(area.height.saturating_sub(2))
        .min((area.height / 2).max(base_detail));
    let table_height = area.height.saturating_sub(detail_height);
    let header_count = if cleanup_view { 2 } else { 1 };
    let visible = table_height.saturating_sub(header_count) as usize;
    let offset = app.selected.saturating_sub(visible.saturating_sub(1));
    let headers: Vec<_> = lines.drain(..header_count as usize).collect();
    let shown = headers
        .into_iter()
        .chain(lines.into_iter().skip(offset).take(visible))
        .collect::<Vec<_>>();
    let shown_count = shown.len() as u16;
    frame.render_widget(
        Paragraph::new(shown),
        Rect {
            height: table_height,
            ..area
        },
    );
    // Use blank space to preview concrete candidates without expanding the category.
    let spare = table_height.saturating_sub(shown_count + 1);
    if spare >= 4
        && let Some(key) = rows
            .get(app.selected)
            .and_then(|r| r.expansion_key.as_deref())
            .filter(|key| key.starts_with("cargo:") || key.starts_with("cleanup:"))
    {
        let path = key.strip_prefix("cargo:").unwrap_or_else(|| {
            key.strip_prefix("cleanup:")
                .unwrap()
                .split_once(':')
                .unwrap()
                .1
        });
        let parent = std::path::Path::new(path);
        let mut candidates: Vec<_> = if key.starts_with("cleanup:") {
            crate::model::cleanup_members(&app.report, key)
        } else {
            app.report
                .nested_artifacts
                .iter()
                .filter(|u| {
                    u.present
                        && u.bytes > 0
                        && u.path != parent
                        && u.path.starts_with(parent)
                        && swamp_core::cargo_cleanup::candidate(u)
                })
                .collect()
        };
        candidates
            .sort_by(|a, b| swamp_core::cargo_cleanup::cleanup_order(a, b, app.report.observed_at));
        if !candidates.is_empty() {
            let capacity = spare.saturating_sub(2) as usize;
            let mut preview = vec![Line::styled(
                format!(
                    "Oldest candidates · showing {} of {} · → expand to select",
                    candidates.len().min(capacity),
                    candidates.len()
                ),
                Style::default().add_modifier(Modifier::BOLD),
            )];
            let path_width = (width / 3).min(64);
            preview.push(Line::raw(format!(
                "{}{:>12}  {:8}  Effect of removal",
                pad_display("Path within category", path_width),
                "Allocated",
                "Modified"
            )));
            preview.extend(candidates.iter().take(capacity).map(|u| {
                Line::raw(format!(
                    "{}{:>12}  {:8}  {}",
                    pad_display(
                        &u.path
                            .strip_prefix(parent)
                            .unwrap_or(&u.path)
                            .display()
                            .to_string(),
                        path_width
                    ),
                    human_bytes(u.bytes),
                    crate::model::age_label(swamp_core::cargo_cleanup::modified_age_secs(
                        u,
                        app.report.observed_at
                    )),
                    match u.role {
                        swamp_core::artifact::ArtifactRole::Incremental => "slower next build",
                        swamp_core::artifact::ArtifactRole::BuildScriptOutput =>
                            "rerun build script",
                        _ => "rebuild before rerunning",
                    }
                ))
            }));
            frame.render_widget(
                Paragraph::new(preview),
                Rect {
                    y: area.y + shown_count + 1,
                    height: spare,
                    ..area
                },
            );
        }
    }
    if let Some(row) = rows.get(app.selected) {
        // #60: the selected row's own decision-evidence lines (activity,
        // consumers, current-use, recovery, reclaimability), below the
        // existing git-status signal line. Dimmed so the signal line
        // (the previously-existing content) stays visually primary.
        let mut detail_lines: Vec<Line> = Vec::new();
        if !row.signals.is_empty() {
            detail_lines.push(Line::raw(row.signals.join(" · ")));
        }
        detail_lines.extend(
            selected_evidence_lines
                .iter()
                .map(|l| Line::styled(l.clone(), Style::default().add_modifier(Modifier::DIM))),
        );
        frame.render_widget(
            Paragraph::new(detail_lines).wrap(ratatui::widgets::Wrap { trim: true }),
            Rect {
                y: area.y + table_height,
                height: detail_height,
                ..area
            },
        );
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let w = area.width.min(90);
    let h = area.height.min(48);
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, popup);
    let mut text = vec![
        Line::from("Keys"),
        Line::from("  ↑↓        move selection"),
        Line::from("  →/←       in / out: open or expand · collapse or go back"),
        Line::from("  Enter     open project / confirm delete"),
        Line::from(
            "  Space     mark / unmark the row
  A         mark every row here the tool can act on
  Backspace delete what is under the cursor (or the marks), asks once
            on a project row, that is every artifact it holds; its checkout, .git and source stay",
        ),
        Line::from(
            "            paths go to Trash; docker images and volumes are removed by the daemon and do not",
        ),
        Line::from("  /         filter picker (form) · : edit filter as text, Tab completes"),
        Line::from("  0         clear filter"),
        Line::from(
            "  v, 1-9    switch view (projects · tree · builds · deps · docker · kinds · unowned ·",
        ),
        Line::from("            types · external); v also reaches agents (0 is clear filter)"),
        Line::from(
            "  g/s/n/t/a sort by growth / size / name / type / age · r reverses (remembered)",
        ),
        Line::from(
            "  k         keep executables: copy target/{release,debug} binaries, dist/*.whl to bin/ before trashing",
        ),
        Line::from("  i         inspect selected Cargo profile dependencies (on demand)"),
        Line::from("  ?         toggle this help"),
        Line::from("  q         quit"),
        Line::from(""),
        Line::from(
            "Columns: bytes · growth in window, then its bar around the centre axis: left green shrank, right red grew, log-scaled, a dim tick below 1MB · facts",
        ),
        Line::from("  [tracked] [ignored] [untracked]: git status; untracked has no copy anywhere"),
        Line::from(""),
        Line::from("Filter grammar"),
        Line::from("  growth [><] <size> in <duration>   (window capped at stored history)"),
        Line::from(
            "  kind:<k>   project:<name|glob*>   type:rs|js|py|go|…   pr:open|merged|closed|none",
        ),
        Line::from("  idle > <duration>   merge-complete   size > <bytes>   age > <duration>"),
        Line::from(""),
        Line::from(
            "Badges  🦀 rs  ⬢ js  🦕 deno  🐍 py  🐹 go  ☕ java  🔺 scala  🔧 cpp  🐦 swift  🟣 net",
        ),
        Line::from(
            "        💎 rb  💧 ex  🐘 php  λ hs  🎯 dart  ⚡ zig  🌍 tf  🐳 docker  🎲 unity  🎮 ue",
        ),
        Line::from("        🔨 has build output   ⎇ N  N linked worktrees"),
    ];
    // The activity-evidence inventory (#54): which domains this pass can
    // establish a real activity fact for, and which it reports as
    // unknown. `docs/usage.md` carries the same table, checked against
    // the constant by `evidence_contract.rs`.
    text.push(Line::from(""));
    text.push(Line::from("Activity evidence this pass can establish"));
    // Wrapped rather than clipped: the clipped tail is where each entry
    // says what the evidence cannot establish.
    let room = usize::from(w.saturating_sub(2)).max(20);
    for (domain, evidence) in swamp_core::activity::ACTIVITY_EVIDENCE_INVENTORY {
        let mut line = String::from(" ");
        for word in format!("{domain}: {evidence}").split_whitespace() {
            if line.chars().count() + 1 + word.chars().count() > room && !line.trim().is_empty() {
                text.push(Line::from(std::mem::replace(
                    &mut line,
                    String::from("   "),
                )));
            }
            line.push(' ');
            line.push_str(word);
        }
        text.push(Line::from(line));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title("help (? to close)");
    frame.render_widget(Paragraph::new(text).block(block), popup);
}

/// Chooses up to `n` signals worth a narrow column: anything that is not
/// the quiet default (`clean`, `0 unpushed`, `unlocked`, `unknown`, `no PR`)
/// first, then the last-commit age.
pub fn pick_signals(signals: &[String], n: usize) -> Vec<String> {
    let quiet = |s: &str| {
        matches!(s, "clean" | "0 unpushed" | "unlocked" | "unknown" | "no PR")
            || s.starts_with("unknown (")
    };
    let mut out: Vec<String> = signals.iter().filter(|s| !quiet(s)).cloned().collect();
    if out.is_empty() {
        out.extend(signals.iter().take(1).cloned());
    }
    out.truncate(n);
    out
}
