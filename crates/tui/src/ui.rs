//! ratatui rendering. Diffstat-ledger world: box-drawing rail, reverse
//! video selection, a `✗` glyph for marked rows, red and green only for
//! signed growth. Emphasis is bold, dim or reverse video, never a color
//! that a light theme can wash out. See DESIGN.md.

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
/// The selected row is drawn in reverse video (plus bold): it follows the
/// terminal's own foreground and background on any theme, survives
/// `NO_COLOR`, and does not depend on telling two colors apart.
fn selected_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

/// Emphasis for a line of status text. Yellow and cyan are unreadable on
/// many light themes (1.7:1 and 2.0:1 against white), so a "warning" is
/// bold and a "note" is plain; red stays red because it also starts with
/// the word that names it (`refused:`, `Blocked:`).
fn tone(c: Color) -> Style {
    match c {
        Color::Yellow => Style::default().add_modifier(Modifier::BOLD),
        Color::Red => Style::default().fg(Color::Red),
        _ => Style::default(),
    }
}

/// Width of every history sparkline, header and rows alike.
const SPARK_WIDTH: u16 = 12;

/// Draws a byte series as a timeline of movement with ratatui's
/// `Sparkline`: each bar is one bucket's change, its height the size of
/// the change relative to the row's largest, red for bytes arriving and
/// green for bytes leaving. A bucket where nothing moved is blank; a
/// bucket before the first observation is a dim `·`.
fn draw_spark(frame: &mut Frame, series: &[Option<u64>], area: Rect) {
    let d = spark_deltas(series, area.width as usize);
    let max = d
        .iter()
        .filter_map(|v| v.map(i64::unsigned_abs))
        .max()
        .unwrap_or(0);
    let bars: Vec<SparklineBar> = d
        .iter()
        .map(|v| match v {
            None => SparklineBar::from(None::<u64>),
            Some(0) => SparklineBar::from(Some(0u64)),
            Some(x) => SparklineBar::from(Some(x.unsigned_abs())).style(Some(
                Style::default().fg(if *x > 0 { GROW } else { SHRINK }),
            )),
        })
        .collect();
    frame.render_widget(
        Sparkline::default()
            .data(bars)
            .max(max.max(1))
            .absent_value_symbol("·")
            .absent_value_style(Style::default().add_modifier(Modifier::DIM)),
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
            format!("{sp} another observation running (pid {}, {e})", h.pid),
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

/// What the clock-driven parts of the screen currently read: the index's
/// age and its warning, and whether a timed refusal is still showing.
/// When this changes the screen is repainted even though nobody touched
/// anything.
pub fn clock_signature(app: &App) -> String {
    format!(
        "{}|{}|{}",
        age_label(app),
        stale_warning(app).is_some(),
        app.refusal_active().is_some()
    )
}

/// The header's warning: the index is older than
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

/// The key legend, shortened to fit `width` cells. Ordered by what a
/// person reaches for first: filter, view, refresh and delete come before
/// movement (arrow keys need no legend). `? help  q quit` is always kept:
/// it is how you find every other key. Items are dropped from the end.
fn footer_legend(width: usize, blocked: bool) -> String {
    const BASE: [&str; 11] = [
        "/ filter",
        "v view",
        "R refresh",
        "⌫ delete",
        "Space mark",
        "A mark all",
        "↑↓ move",
        "→/← in/out",
        "g/s/n/t/a sort",
        "r reverse",
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
        .map(|(l, c)| Line::styled(l.clone(), tone(*c)))
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
                    .style(tone(Color::Yellow)),
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
                .style(Style::default()),
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
            // Two rows per item.
            app.page
                .set((usize::from(SHEET_ROWS.min(chunks[2].height)).saturating_sub(2) / 2).max(1));
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
            "↑↓ scroll · r check again · Esc back to the plan".to_string()
        } else {
            "↑↓ scroll · r check again · Esc close".to_string()
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
        fit_clauses(
            &[
                "↑↓ field".to_string(),
                "←→ value".to_string(),
                "Enter apply".to_string(),
                "Esc cancel".to_string(),
                "e edit as text".to_string(),
                "0 clear".to_string(),
            ],
            size.width as usize,
        )
    } else if app.editing_filter {
        "Tab complete · Enter apply · Esc cancel".to_string()
    } else {
        footer_legend(size.width as usize, !app.blocked.is_empty())
    };
    frame.render_widget(Paragraph::new(footer_text), chunks[4]);

    if app.help_open {
        draw_help(frame, app, size);
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
        // Long lines wrap under themselves; nothing is cut at the edge.
        let inner_w = usize::from(popup.width.saturating_sub(2));
        let wrapped: Vec<String> = lines
            .iter()
            .flat_map(|l| wrap_hanging(l, inner_w, 2))
            .collect();
        let inner_h = usize::from(popup.height.saturating_sub(2));
        app.page.set(inner_h.saturating_sub(1).max(1));
        let visible: Vec<Line> = wrapped
            .iter()
            .skip(app.cargo_inspection_scroll as usize)
            .take(inner_h)
            .map(|s| Line::from(s.as_str()))
            .collect();
        frame.render_widget(
            Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(
                " Cargo dependency inspection · ↑↓ PgUp PgDn scroll · Esc close · no cleanup action ",
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
    // The keys live in the footer, once; the box holds only the form.
    lines.push(Line::from(format!("  filter: {composed}    → {count}")));
    let block = Block::default().borders(Borders::ALL).title(" filter ");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Header: facts on the left, the whole root's history on the right as a
/// sparkline with its net change and the window it covers.
fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let total = &app.report.total_series;
    let net = net_change(total);
    // The number says what it measures: net change over the window the
    // history covers, e.g. `-41.4GB in 1w`.
    let net_text = net.map(|d| {
        if app.report.series_window_secs > 0 {
            format!(
                "{} in {}",
                human_signed_bytes(d),
                human_duration(app.report.series_window_secs)
            )
        } else {
            human_signed_bytes(d)
        }
    });
    let right_width: u16 = match &net_text {
        Some(t) if area.width >= 80 => SPARK_WIDTH + 1 + crate::model::display_width(t) as u16,
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
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(text[i + n..].to_string(), dim),
        ]),
        None => Line::from(Span::styled(text, dim)),
    };
    frame.render_widget(Paragraph::new(line), left);
    if let Some(text) = net_text.filter(|_| right_width > 0) {
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
        );
        frame.render_widget(
            Paragraph::new(text).style(Style::default().add_modifier(Modifier::DIM)),
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
        // Which view this is, where it sits among the ten, and how to move:
        // `v` walks the list in order, Esc goes to the projects list.
        let place = format!(
            "{} of {}",
            app.view.position(),
            crate::app::ViewKind::ALL.len()
        );
        let scope = match (app.view, app.selected_project.as_deref()) {
            (crate::app::ViewKind::Projects, _) => format!("projects ({place} · v next)"),
            (crate::app::ViewKind::Tree, Some(p)) => {
                format!("tree of {p} ({place} · Esc: projects)")
            }
            (v, Some(p)) => format!("{} of {p} ({place} · v next · Esc: projects)", v.label()),
            (v, None) => format!("{} ({place} · v next · Esc: projects)", v.label()),
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

/// What an empty list says: why it is empty and what to press next.
fn empty_state(app: &App) -> String {
    use crate::app::ViewKind as V;
    // Only these views read the filter; the others are never emptied by it.
    let filtered = matches!(
        app.view,
        V::Projects | V::Tree | V::Builds | V::Deps | V::Kinds | V::Types
    );
    if filtered && app.filter_text != "0" {
        return format!(
            "Nothing matches the filter \"{}\". Press / to change it, or 0 to clear it and see everything.",
            app.filter_text
        );
    }
    match app.view {
        V::Projects if !app.has_index => {
            "Nothing has been scanned yet. Press R to scan; it runs in the background.".to_string()
        }
        V::Projects => format!(
            "No projects found under {}. Press R to scan again.",
            app.root.display()
        ),
        V::Tree => {
            "Nothing to show for this project. Esc goes back to the project list.".to_string()
        }
        V::Builds => {
            "No build output found. Press v for another view, or R to scan again.".to_string()
        }
        V::Deps => {
            "No dependency folders found. Press v for another view, or R to scan again.".to_string()
        }
        V::Docker => {
            "No Docker images, containers or volumes found. Press v for another view.".to_string()
        }
        V::Agents => {
            "No AI-tool storage found. Press v for another view, or R to scan again.".to_string()
        }
        _ => "Nothing here yet. Press v for another view, or R to scan again.".to_string(),
    }
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    let rows = app.rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_state(app)).wrap(ratatui::widgets::Wrap { trim: true }),
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
        // The tree rail is never cut: only what follows it gives way, so a
        // narrow terminal keeps the outline readable.
        let rail_width = crate::model::display_width(&row.rail);
        let rest = format!("{mark_prefix}{}{badge}{track}", row.label);
        let name = format!(
            "{}{}",
            row.rail,
            truncate_middle(&rest, name_width.saturating_sub(rail_width).max(1))
        );
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
            _ => Color::Reset,
        };
        // A change too small to act on is dimmed, number and tick alike.
        let bar_style = if is_noise(row.growth) || row.growth.is_none_or(|g| g == 0) {
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
            Style::default().add_modifier(Modifier::BOLD)
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

        let mut spans = spans;
        if i == app.selected {
            // No color inside the reverse-video bar: a colored span would
            // turn into a colored block, and under NO_COLOR the terminal
            // is told to reset at each one, cutting the bar short. The
            // sign of the change is in the number.
            for sp in &mut spans {
                sp.style.fg = None;
            }
            // The bar runs the full width, not only as far as the text.
            let used: usize = spans
                .iter()
                .map(|s| crate::model::display_width(&s.content))
                .sum();
            if used < width {
                spans.push(Span::raw(" ".repeat(width - used)));
            }
        }
        let mut line = Line::from(spans);
        if i == app.selected {
            line = line.style(selected_style());
        }
        lines.push(line);
    }
    // The detail pane is the same height whichever row is selected, so
    // the table never resizes under the cursor.
    let detail_height = DETAIL_ROWS.min(area.height / 3);
    let table_height = area.height.saturating_sub(detail_height);
    let header_count = if cleanup_view { 2 } else { 1 };
    let visible = table_height.saturating_sub(header_count) as usize;
    // One page is a screenful with a row of overlap.
    app.page.set(visible.saturating_sub(1).max(1));
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
        // What the row is, what rebuilding costs, and only the facts that
        // change a decision (crate::detail).
        let path = row
            .unit
            .as_ref()
            .map(|u| std::path::Path::new(&u.0))
            .or_else(|| row.worktree.as_ref().map(|w| w.path.as_path()));
        let sharing = path.map_or_else(Vec::new, |p| {
            swamp_core::render::render_sharing_lines(
                app.report.reconciliation.unique_estimate.as_ref(),
                Some(p),
            )
        });
        let detail_lines: Vec<Line> = crate::detail::lines(row, &sharing)
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    Line::raw(l)
                } else {
                    Line::styled(l, Style::default().add_modifier(Modifier::DIM))
                }
            })
            .collect();
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

/// Wraps `text` to `width` cells at spaces, keeping its own spacing;
/// continuation lines start with `hang` spaces so a wrapped entry stays
/// under its own description.
fn wrap_hanging(text: &str, width: usize, hang: usize) -> Vec<String> {
    let width = width.max(hang + 8);
    let mut out: Vec<String> = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    loop {
        let total: usize = rest
            .iter()
            .map(|c| crate::model::display_width(&c.to_string()))
            .sum();
        if total <= width {
            out.push(rest.iter().collect());
            return out;
        }
        // The last space that still leaves the line within `width`.
        let mut cells = 0usize;
        let mut brk = None;
        let mut cut = rest.len();
        for (i, c) in rest.iter().enumerate() {
            cells += crate::model::display_width(&c.to_string());
            if cells > width {
                cut = i;
                break;
            }
            if *c == ' ' && i > hang {
                brk = Some(i);
            }
        }
        let at = brk.unwrap_or(cut.max(1));
        let head: String = rest[..at].iter().collect();
        out.push(head.trim_end().to_string());
        let tail: String = rest[at..].iter().collect();
        rest = format!("{}{}", " ".repeat(hang), tail.trim_start())
            .chars()
            .collect();
    }
}

/// The help text as `(line, is_heading)`, wrapped to `width`. One key per
/// entry, one entry per line: nothing runs together and nothing is cut.
fn help_lines(app: &App, width: usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let heading = |out: &mut Vec<(String, bool)>, t: &str| out.push((t.to_string(), true));
    let blank = |out: &mut Vec<(String, bool)>| out.push((String::new(), false));
    // Key column of 11 cells, then the description, wrapped under itself.
    let entry = |out: &mut Vec<(String, bool)>, key: &str, desc: &str| {
        let text = format!("  {}{desc}", pad_display(key, 11));
        for l in wrap_hanging(&text, width, 13) {
            out.push((l, false));
        }
    };
    let plain = |out: &mut Vec<(String, bool)>, text: &str, hang: usize| {
        for l in wrap_hanging(text, width, hang) {
            out.push((l, false));
        }
    };
    heading(&mut out, "Keys");
    entry(&mut out, "↑ ↓", "move the cursor");
    entry(
        &mut out,
        "PgUp PgDn",
        "move a screenful · Home and End jump to the first and last row",
    );
    entry(
        &mut out,
        "→ / ←",
        "in and out: open or expand · collapse or go back",
    );
    entry(&mut out, "Enter", "open the project · on the plan, confirm");
    entry(
        &mut out,
        "Space",
        "mark or unmark the row. On a project row: everything in it that can be rebuilt",
    );
    entry(&mut out, "A", "mark every row here that can be cleaned up");
    entry(
        &mut out,
        "Backspace",
        "move what is under the cursor (or everything marked) to Trash, after one confirm",
    );
    entry(
        &mut out,
        "",
        "On a project row that is its rebuildable items; only if it has none, the checkout itself (named 'checkout' on the plan), with .git and source, into Trash.",
    );
    entry(
        &mut out,
        "",
        "Docker images and volumes are removed by docker for good: no Trash.",
    );
    entry(
        &mut out,
        "b  d",
        "list what the last check could not include, with the reason and the next step (d on the plan)",
    );
    entry(
        &mut out,
        "✗  ~n/m",
        "a project row is all marked, or n of its m items are",
    );
    entry(
        &mut out,
        "/",
        "filter picker (a form) · : edits the filter as text, Tab completes · 0 clears it",
    );
    entry(
        &mut out,
        "v  1-9",
        "next view, or pick one: 1 projects 2 tree 3 builds 4 deps 5 docker 6 kinds 7 unowned 8 types 9 external. v also reaches agents. Esc returns to projects.",
    );
    entry(
        &mut out,
        "g s n t a",
        "sort by growth, size, name, type, age; press again to turn the sort off · r reverses it · remembered",
    );
    entry(
        &mut out,
        "k",
        &format!(
            "keep executables: {} now. Copies release and debug programs and dist wheels to bin/ before their folder goes to Trash. Remembered for next time; the result line says which way it went.",
            if app.keep_executables { "ON" } else { "OFF" }
        ),
    );
    entry(
        &mut out,
        "i",
        "inspect the Cargo dependencies of the selected profile",
    );
    entry(
        &mut out,
        "R",
        "refresh: scan again in the background. Opening never scans when an index exists",
    );
    entry(&mut out, "?", "this help · Esc or q closes it");
    entry(
        &mut out,
        "q",
        "quit. While a check or a move runs, q and Esc stop it after the current item",
    );
    blank(&mut out);
    heading(&mut out, "Columns");
    plain(
        &mut out,
        "  Size, then growth over the window with its bar around the centre axis: left green shrank, right red grew, log-scaled, a dim tick below 1MB. Then facts.",
        2,
    );
    plain(
        &mut out,
        "  [tracked] [ignored] [untracked] is git status; untracked has no copy anywhere.",
        2,
    );
    blank(&mut out);
    heading(&mut out, "Filter grammar");
    plain(
        &mut out,
        "  growth [><] <size> in <duration>   (window capped at stored history)",
        4,
    );
    plain(
        &mut out,
        "  kind:<k>   project:<name|glob*>   type:rs|js|py|go|…   pr:open|merged|closed|none",
        4,
    );
    plain(
        &mut out,
        "  idle > <duration>   merge-complete   size > <bytes>   age > <duration>",
        4,
    );
    blank(&mut out);
    heading(&mut out, "Badges");
    for l in [
        "  🦀 rs  ⬢ js  🦕 deno  🐍 py  🐹 go  ☕ java  🔺 scala  🔧 cpp  🐦 swift  🟣 net",
        "  💎 rb  💧 ex  🐘 php  λ hs  🎯 dart  ⚡ zig  🌍 tf  🐳 docker  🎲 unity  🎮 ue",
        "  🔨 has build output   ⎇ N  N linked worktrees",
    ] {
        plain(&mut out, l, 2);
    }
    // The activity-evidence inventory (#54): which domains this pass can
    // establish a real activity fact for, and which it reports as
    // unknown. `docs/usage.md` carries the same table, checked against
    // the constant by `evidence_contract.rs`.
    blank(&mut out);
    heading(&mut out, "Activity evidence this pass can establish");
    for (domain, evidence) in swamp_core::activity::ACTIVITY_EVIDENCE_INVENTORY {
        plain(&mut out, &format!("  {domain}: {evidence}"), 4);
    }
    out
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let w = area.width.min(96);
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: area.y,
        width: w,
        height: area.height,
    };
    frame.render_widget(Clear, popup);
    let inner_w = usize::from(w.saturating_sub(2));
    let inner_h = usize::from(popup.height.saturating_sub(2));
    let lines = help_lines(app, inner_w);
    // Clamp here, once per frame: End may ask for "as far as it goes".
    let last = lines.len().saturating_sub(inner_h);
    let top = app.help_scroll.get().min(last);
    app.help_scroll.set(top);
    app.page.set(inner_h.saturating_sub(1).max(1));
    let shown: Vec<Line> = lines
        .iter()
        .skip(top)
        .take(inner_h)
        .map(|(l, head)| {
            if *head {
                Line::styled(l.clone(), Style::default().add_modifier(Modifier::BOLD))
            } else {
                Line::from(l.clone())
            }
        })
        .collect();
    let title = format!(
        " help · ↑↓ PgUp PgDn Home End scroll · Esc closes · {}-{} of {} ",
        (top + 1).min(lines.len()),
        (top + inner_h).min(lines.len()),
        lines.len()
    );
    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(Paragraph::new(shown).block(block), popup);
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
