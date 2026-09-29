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
    // While our own walk runs, the chip carries the state and elapsed
    // time; the only detail after it is how much has been seen.
    let obs = if app.observing.is_some() {
        let (bytes, _, _) = swamp_core::walk::progress::snapshot();
        let roots = if app.roots.len() > 1 {
            format!(" · {} roots", app.roots.len())
        } else {
            String::new()
        };
        format!("{} seen{roots}", human_bytes(bytes))
    } else {
        // The age comes from the report itself, so it keeps counting
        // while the UI stays open.
        format!("observed {}", age_label(app))
    };
    let warn = stale_warning(app);
    let since = since_label(app)
        .map(|s| format!(" · since {s}"))
        .unwrap_or_default();
    // Clauses in priority order; the renderer drops trailing clauses that
    // do not fit the terminal width rather than truncating mid-word.
    let clauses = vec![
        app.disk_banner.clone().unwrap_or_default(),
        app.status.clone().unwrap_or_default(),
        warn.clone().unwrap_or_default(),
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
        if warn.is_some() {
            since.trim_start_matches(" · ").to_string()
        } else {
            format!("{obs}{since}")
        },
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
    // The chip owns the left edge at every width; the clauses share what
    // is left and are the ones that give way.
    match activity_chip(app, width) {
        Some(chip) => {
            let room = width.saturating_sub(crate::model::display_width(&chip) + 3);
            if room >= 8 {
                format!("{chip} · {}", fit_clauses(&clauses, room))
            } else {
                chip
            }
        }
        None => fit_clauses(&clauses, width),
    }
}

/// One braille frame per redraw (`App::frame` counts them): the event
/// loop redraws every 200 ms, so a busy screen is never still.
fn spinner_frame(app: &App) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(app.frame % 10) as usize]
}

/// The header's fixed activity slot: something is scanning, and for how
/// long. It is the first thing on the line and comes in shorter forms, so
/// it fits at any width instead of being dropped with the tail.
fn activity_chip(app: &App, width: usize) -> Option<String> {
    let sp = spinner_frame(app);
    let tiers: Vec<String> = if app.observing.is_some() {
        let secs = app.observing_started.map_or(0, |t| t.elapsed().as_secs());
        let e = swamp_core::schedule::format_elapsed(secs);
        vec![format!("{sp} observing {e}"), format!("{sp} {e}")]
    } else {
        let h = app.external_observer?;
        let secs = swamp_core::entities::now().saturating_sub(h.since);
        let e = swamp_core::schedule::format_elapsed(secs);
        vec![
            format!("{sp} scheduled observation running (pid {}, {e})", h.pid),
            format!("{sp} observation running {e}"),
            format!("{sp} observing {e}"),
            format!("{sp} {e}"),
        ]
    };
    let last = tiers.last().cloned();
    tiers
        .into_iter()
        .find(|t| crate::model::display_width(t) <= width)
        .or(last)
}

/// Joins clauses with " · " while the result fits in `width`; always keeps
/// the first clause, truncated to terminal cells if necessary.
/// How old the index is, from its own `observed_at` (so it keeps
/// counting while the UI stays open and resets when a refresh lands).
fn age_label(app: &App) -> String {
    if app.live_age && app.report.observed_at > 0 {
        swamp_core::schedule::format_ago(swamp_core::entities::now(), app.report.observed_at)
    } else {
        app.observed_label.clone()
    }
}

/// The header's yellow warning: the index is older than
/// `STALE_AFTER_SECS` (or missing) and nothing is scanning. While a scan
/// -- ours or the scheduled one -- is running, its own indicator
/// replaces this hint.
pub fn stale_warning(app: &App) -> Option<String> {
    if app.observing.is_some() || app.external_observer.is_some() || app.disk_banner.is_some() {
        return None;
    }
    if !app.has_index {
        return Some("no index yet · press R to scan".to_string());
    }
    let observed = app.report.observed_at;
    if !app.live_age || observed == 0 {
        return None;
    }
    let age = swamp_core::entities::now().saturating_sub(observed);
    (age > crate::app::STALE_AFTER_SECS).then(|| {
        format!(
            "observed {} · older than {} min, press R to refresh",
            swamp_core::schedule::format_ago(swamp_core::entities::now(), observed),
            crate::app::STALE_AFTER_SECS / 60
        )
    })
}

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

/// The key legend, shortened to fit `width` cells. `? help  q quit`
/// is always kept: it is how you find every other key.
fn footer_legend(width: usize, blocked: bool) -> String {
    const BASE: [&str; 12] = [
        "↑↓ move",
        "→/← in/out",
        "Enter open/confirm",
        "Space mark",
        "A mark all",
        "⌫ delete",
        "/ filter",
        "v view",
        "g/s/n/t/a sort",
        "r reverse",
        "R refresh",
        "? help",
    ];
    const TAIL: &str = "q quit";
    let mut items: Vec<&str> = BASE.to_vec();
    if blocked {
        // What the last check could not include, one key from the list.
        items.insert(4, "b blocked");
    }
    let all_items = items;
    let mut n = all_items.len();
    loop {
        let mut parts: Vec<&str> = all_items[..n].to_vec();
        // Keep `? help` even when the middle is cut.
        if n < all_items.len() {
            parts.push(all_items[all_items.len() - 1]);
        }
        parts.push(TAIL);
        let line = parts.join("  ");
        if line.chars().count() <= width || n == 0 {
            return line;
        }
        n -= 1;
    }
}

/// Rows the status region always takes: what is happening, or what just
/// happened, or what is about to. Never more, never fewer.
const STATUS_ROWS: u16 = 2;
/// Rows of the plan and blocked sheets, borders included. A sheet covers
/// the bottom of the table; it never changes the table's height.
const SHEET_ROWS: u16 = 10;
/// Rows of the detail pane under the table, whichever row is selected.
const DETAIL_ROWS: u16 = 4;

/// Cuts `s` to `width` cells at the end, marking the cut with `…`.
fn clip_end(s: &str, width: usize) -> String {
    if crate::model::display_width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if crate::model::display_width(&out) + crate::model::display_width(&c.to_string()) + 1
            > width
        {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// Whole lines that fit `cap` rows once wrapped at `width`; what does not
/// fit is counted in a last line instead of being cut mid-sentence.
fn fit_lines(
    lines: &[(String, Color)],
    width: usize,
    cap: usize,
    more: &dyn Fn(usize) -> String,
) -> Vec<(String, Color)> {
    let mut used = 0usize;
    let mut fit: Vec<(String, Color)> = Vec::new();
    for (i, (l, c)) in lines.iter().enumerate() {
        let rows = wrapped_rows(l, width);
        let left = lines.len() - i - 1;
        // Keep one row for the "+N more" line when something is left over.
        let reserve = usize::from(left > 0);
        if used + rows + reserve > cap && !fit.is_empty() {
            fit.push((more(left + 1), Color::Yellow));
            return fit;
        }
        used += rows;
        fit.push((l.clone(), *c));
    }
    fit
}

/// Rows a greedy word wrap of `text` takes at `width` columns.
fn wrapped_rows(text: &str, width: usize) -> usize {
    let width = width.max(1);
    let mut rows = 1usize;
    let mut cur = 0usize;
    for word in text.split_whitespace() {
        let wl = word.chars().count();
        if cur == 0 {
            cur = wl;
        } else if cur + 1 + wl <= width {
            cur += 1 + wl;
        } else {
            rows += 1;
            cur = wl;
        }
        while cur > width {
            rows += 1;
            cur -= width;
        }
    }
    rows
}

fn items(n: usize) -> String {
    if n == 1 {
        "1 item".to_string()
    } else {
        format!("{n} items")
    }
}

/// The plan sheet: what is ready and what is blocked, the plan by project,
/// what cannot come back, then names and warnings. `summary` is the
/// confirm text; its first line is the headline shown in the status rows.
fn plan_sheet(app: &App, summary: &[String]) -> Vec<(String, Color)> {
    let rest: &[String] = summary.get(1..).unwrap_or(&[]);
    let irreversible = rest
        .iter()
        .take_while(|l| l.starts_with("Remove ") || l.starts_with("Gone for good"))
        .count();
    let yellow = |l: &String| (l.clone(), Color::Yellow);
    let mut out: Vec<(String, Color)> = rest[..irreversible].iter().map(yellow).collect();
    let ready = app.marked.len();
    let blocked = app.blocked.len();
    out.push((
        if blocked == 0 {
            format!("{ready} ready · none blocked")
        } else {
            format!("{ready} ready · {blocked} blocked (d to see why)")
        },
        Color::Reset,
    ));
    if let Some(first) = app.blocked.first() {
        let more = app
            .blocked
            .iter()
            .filter(|b| b.reason != first.reason)
            .map(|b| &b.reason)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let more = if more > 0 {
            format!(" (+{more} more reasons)")
        } else {
            String::new()
        };
        out.push((format!("Blocked: {}{more}", first.reason), Color::Red));
    }
    let in_use = app
        .marked
        .values()
        .filter(|u| u.warnings.iter().any(|w| w == "currently in use"))
        .count();
    if in_use > 0 {
        out.push((
            format!(
                "{} in use: a process has them open. Close them and check again.",
                items(in_use)
            ),
            Color::Yellow,
        ));
    }
    out.extend(project_breakdown(app));
    out.extend(rest[irreversible..].iter().map(yellow));
    out
}

/// The plan by project, largest first, the tail folded into one line.
fn project_breakdown(app: &App) -> Vec<(String, Color)> {
    let mut by: std::collections::BTreeMap<String, (u64, usize)> = Default::default();
    for u in app.marked.values() {
        let name = if u.docker.is_some() {
            "docker".to_string()
        } else {
            crate::names::project_of(&app.report, &u.path).unwrap_or_else(|| "other".to_string())
        };
        let e = by.entry(name).or_default();
        e.0 += u.bytes;
        e.1 += 1;
    }
    let mut v: Vec<(String, (u64, usize))> = by.into_iter().collect();
    v.sort_by_key(|(_, (b, _))| std::cmp::Reverse(*b));
    let mut out: Vec<(String, Color)> = v
        .iter()
        .take(3)
        .map(|(n, (b, c))| {
            (
                format!("  {n}  {}  {}", human_bytes(*b), items(*c)),
                Color::Reset,
            )
        })
        .collect();
    if v.len() > 3 {
        let (b, c) = v[3..]
            .iter()
            .fold((0u64, 0usize), |a, (_, (b, c))| (a.0 + b, a.1 + c));
        out.push((
            format!("  + {} more  {}  {}", v.len() - 3, human_bytes(b), items(c)),
            Color::Reset,
        ));
    }
    out
}

/// The blocked sheet: each item, why, and what to do next.
fn blocked_sheet(app: &App) -> Vec<(String, Color)> {
    let mut out = Vec::new();
    for b in app.blocked.iter().skip(app.blocked_scroll) {
        out.push((format!("{}  {}", b.name, b.reason), Color::Reset));
        out.push((format!("  next: {}", b.next), Color::Yellow));
    }
    out
}

/// A sheet over the bottom of the body: bordered, fixed height, whole
/// lines with a count of what did not fit.
fn draw_sheet(
    frame: &mut Frame,
    body: Rect,
    title: &str,
    lines: &[(String, Color)],
    more: &dyn Fn(usize) -> String,
) {
    let h = SHEET_ROWS.min(body.height);
    if h < 3 {
        return;
    }
    let area = Rect {
        y: body.y + body.height - h,
        height: h,
        ..body
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    let inner = block.inner(area);
    let fit = fit_lines(lines, inner.width as usize, inner.height as usize, more);
    let text: Vec<Line> = fit
        .iter()
        .map(|(l, c)| Line::styled(l.clone(), Style::default().fg(*c)))
        .collect();
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

/// The two status rows while something runs: elapsed and a moving glyph
/// first, then what is happening, then the item being worked on.
fn operation_rows(app: &App, op: &crate::app::Operation) -> [String; 2] {
    let elapsed = op.started.elapsed();
    let chip = format!(
        "{} {}",
        spinner_frame(app),
        swamp_core::schedule::format_elapsed(elapsed.as_secs())
    );
    let cancelling = op.cancel.load(std::sync::atomic::Ordering::SeqCst);
    let current = if op.current.is_empty() {
        "starting…".to_string()
    } else {
        op.current.clone()
    };
    match op.label {
        "Reviewing" if cancelling => [
            format!("{chip}  Stopping after this item"),
            "Marks are kept. Nothing has been changed.".to_string(),
        ],
        "Reviewing" if op.checking_open_files.is_some() => [
            format!("{chip}  Checking what is in use"),
            "Marks are kept. Nothing has been changed.".to_string(),
        ],
        "Reviewing" => {
            let counts = if op.total > 0 {
                format!("Checked {} of {}", op.completed, op.total)
            } else {
                format!("Checked {}", op.completed)
            };
            [
                format!(
                    "{chip}  {counts} · {} ready · {} blocked",
                    op.succeeded, op.failed
                ),
                current,
            ]
        }
        "Deleting" => [
            format!(
                "{chip}  {}  {} of {} · {} of {}",
                if cancelling {
                    "Stopping after this item"
                } else {
                    "Moving to Trash"
                },
                op.completed,
                op.total,
                human_bytes(op.bytes_done),
                human_bytes(op.bytes_total)
            ),
            current,
        ],
        _ => [format!("{chip}  {}", op.label), {
            let _ = app;
            current
        }],
    }
}

fn draw_status(frame: &mut Frame, app: &App, summary: &[String], area: Rect) {
    let w = area.width as usize;
    if let Some(op) = &app.operation {
        let rows = operation_rows(app, op);
        let lines: Vec<Line> = rows.iter().map(|r| Line::from(clip_end(r, w))).collect();
        frame.render_widget(Paragraph::new(lines), area);
    } else if app.confirm_open {
        if let Some(headline) = summary.first() {
            frame.render_widget(
                Paragraph::new(headline.clone())
                    .wrap(ratatui::widgets::Wrap { trim: true })
                    .style(Style::default().fg(Color::Yellow)),
                area,
            );
        }
    } else if let Some(msg) = app.refusal_active() {
        frame.render_widget(
            Paragraph::new(msg.to_string())
                .wrap(ratatui::widgets::Wrap { trim: true })
                .style(Style::default().fg(Color::Red)),
            area,
        );
    } else if let Some(r) = app.result_active() {
        frame.render_widget(
            Paragraph::new(r.to_string())
                .wrap(ratatui::widgets::Wrap { trim: true })
                .style(Style::default().fg(Color::Cyan)),
            area,
        );
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let summary: Vec<String> = if app.confirm_open && app.operation.is_none() {
        app.confirm_summary().lines().map(str::to_string).collect()
    } else {
        Vec::new()
    };
    // Chrome is the same five rows in every state: header, filter line,
    // two status rows, keys. Sheets overlay the body; nothing resizes it.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // filter line
            Constraint::Min(1),    // body
            Constraint::Length(STATUS_ROWS),
            Constraint::Length(1), // keys
        ])
        .split(size);

    draw_header(frame, app, chunks[0]);
    draw_filter_line(frame, app, chunks[1]);
    draw_body(frame, app, chunks[2]);
    if app.operation.is_none() {
        if app.blocked_open {
            draw_sheet(
                frame,
                chunks[2],
                &if app.blocked_scroll > 0 {
                    format!(
                        "Blocked: {} · from item {}",
                        app.blocked.len(),
                        app.blocked_scroll + 1
                    )
                } else {
                    format!("Blocked: {}", app.blocked.len())
                },
                &blocked_sheet(app),
                &|_| "more below (↓ to scroll)".to_string(),
            );
        } else if app.confirm_open {
            draw_sheet(
                frame,
                chunks[2],
                "Plan · nothing has changed yet",
                &plan_sheet(app, &summary),
                &|n| format!("+{n} more lines"),
            );
        }
    }
    draw_status(frame, app, &summary, chunks[3]);

    // The keys row is the legend for the state you are actually in, and
    // nothing else ever replaces it.
    let footer_text = if let Some(op) = &app.operation {
        match op.label {
            "Reviewing" => {
                "Esc cancel · Nothing has been changed · Next: review, then Enter to move to Trash"
                    .to_string()
            }
            "Deleting" => "Esc stop after this item · moved items stay in Trash".to_string(),
            _ => "Esc cancel".to_string(),
        }
    } else if app.blocked_open {
        if app.confirm_open {
            "↑↓ scroll · Esc back to the plan".to_string()
        } else {
            "↑↓ scroll · Esc close".to_string()
        }
    } else if app.confirm_open {
        let mut clauses = vec!["Enter confirm".to_string(), "Esc back".to_string()];
        if !app.blocked.is_empty() {
            clauses.push("d blocked".to_string());
        }
        clauses.push(if app.keep_executables {
            "keep executables → bin/ (k)".to_string()
        } else {
            "k keep executables".to_string()
        });
        fit_clauses(&clauses, size.width as usize)
    } else if app.picker.is_some() {
        "↑↓ field · ←→ value · Space grew/shrank · type to narrow project · Enter apply · Esc cancel · e edit as text · 0 clear".to_string()
    } else if app.editing_filter {
        "Tab complete · Enter apply · Esc cancel".to_string()
    } else {
        footer_legend(size.width as usize, !app.blocked.is_empty())
    };
    frame.render_widget(Paragraph::new(footer_text), chunks[4]);

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
    let text = header_line(app, left.width as usize);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let line = match stale_warning(app).and_then(|w| text.find(&w).map(|i| (i, w.len()))) {
        // The warning is yellow and not dimmed; the rest stays quiet.
        Some((i, n)) => Line::from(vec![
            Span::styled(text[..i].to_string(), dim),
            Span::styled(
                text[i..i + n].to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text[i + n..].to_string(), dim),
        ]),
        None => Line::from(Span::styled(text, dim)),
    };
    frame.render_widget(Paragraph::new(line), left);
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
    let mark_states = app.project_mark_states();
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
        // A projects-view row stands for many units: `✗` when all are
        // marked, `~n/m` when some are.
        let mut mark_prefix = if marked {
            "✗ ".to_string()
        } else {
            String::new()
        };
        if row.unit.is_none()
            && row.kind.is_none()
            && row.expansion_key.is_none()
            && let Some(project) = row.project.as_deref()
        {
            let (n, of) = mark_states.get(project).copied().unwrap_or((0, 0));
            if of > 0 && n == of {
                mark_prefix = "✗ ".to_string();
            } else if n > 0 {
                mark_prefix = format!("~{n}/{of} ");
            }
        }
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
    // The detail pane is the same height whichever row is selected, so
    // the table never resizes under the cursor.
    let detail_height = DETAIL_ROWS.min(area.height / 3);
    let table_height = area.height.saturating_sub(detail_height);
    let header_count = if cleanup_view { 2 } else { 1 };
    let visible = table_height.saturating_sub(header_count) as usize;
    // Stateful window: it moves only when the selection leaves it, so one
    // keypress moves the selection one row.
    let mut offset = app.scroll_offset.get();
    if app.selected < offset {
        offset = app.selected;
    } else if visible > 0 && app.selected >= offset + visible {
        offset = app.selected + 1 - visible;
    }
    offset = offset.min(rows.len().saturating_sub(visible));
    app.scroll_offset.set(offset);
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
        // One fact per row, cut at the edge: the front of each fact is
        // the part that decides, and the pane never grows.
        let clipped: Vec<Line> = detail_lines
            .into_iter()
            .take(detail_height as usize)
            .map(|l| {
                let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                Line::styled(clip_end(&text, width), l.style)
            })
            .collect();
        frame.render_widget(
            Paragraph::new(clipped),
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
            "  Space     mark / unmark the row; on a project row, everything rebuildable in it",
        ),
        Line::from("  A         mark every row here the tool can act on"),
        Line::from("  Backspace delete what is under the cursor (or the marks), asks once"),
        Line::from(
            "            on a project row: its rebuildable artifacts; only if it has none, its checkout",
        ),
        Line::from(
            "            (named 'checkout' in the confirm, with .git and source, into Trash)",
        ),
        Line::from(
            "            paths go to Trash; docker images and volumes are removed by the daemon and do not",
        ),
        Line::from("  ✗ / ~n/m  a project row is all marked / n of m units marked"),
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
        Line::from("  ?         toggle this help · R refresh now (background scan)"),
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
