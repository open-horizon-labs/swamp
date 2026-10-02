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
    let warn = stale_warning(app);
    let observed = if app.observing.is_some() {
        format!(
            "updating {} root{}",
            swamp_core::render::human_count(app.roots.len() as u64),
            if app.roots.len() == 1 { "" } else { "s" }
        )
    } else if warn.is_none() {
        format!("observed {}", age_label(app))
    } else {
        String::new()
    };
    // The headline owns totals. This line owns scope, freshness and activity.
    let clauses = vec![
        app.disk_banner.clone().unwrap_or_default(),
        app.status.clone().unwrap_or_default(),
        if app.new_data_waiting() {
            "new data available".into()
        } else {
            String::new()
        },
        warn.unwrap_or_default(),
        if stale
            || app
                .report
                .reconciliation
                .unique_estimate
                .as_ref()
                .is_some_and(|u| u.needs_reconciliation)
        {
            if width >= 120 {
                "unique totals not recomputed (needs reconciliation)".into()
            } else {
                "unique totals not recomputed".into()
            }
        } else {
            String::new()
        },
        app.scope_note.clone().unwrap_or_default(),
        observed,
        app.root.display().to_string(),
        since_label(app)
            .map(|s| format!("change over {s}"))
            .unwrap_or_default(),
        app.declared_note.clone().unwrap_or_default(),
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
        // Under a minute reads "just now": a label that counted seconds
        // would repaint an idle screen once a second.
        let now = swamp_core::entities::now();
        if now.saturating_sub(app.report.observed_at) < 60 {
            "just now".to_string()
        } else {
            swamp_core::schedule::format_ago(now, app.report.observed_at)
        }
    } else {
        app.observed_label.clone()
    }
}

/// What the clock-driven parts of the screen currently read: the index's
/// age and its warning, and whether a timed refusal is still showing.
/// When this changes the screen is repainted even though nobody touched
/// anything.
pub fn clock_signature(app: &App) -> String {
    // The headline block's ages read in minutes and hours; they change on
    // the clock, like the header's own age.
    let now = swamp_core::entities::now();
    let ledger_age = match &app.ledger {
        swamp_core::volume_ledger::LedgerReading::Measured(a) => {
            swamp_core::volume_ledger::age_text(a.measured_at, now)
        }
        _ => String::new(),
    };
    format!(
        "{}|{}|{}|{}|{}",
        age_label(app),
        stale_warning(app).is_some(),
        app.refusal_active().is_some(),
        swamp_core::volume_ledger::age_text(app.report.observed_at, now),
        ledger_age
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
    if let Some(n) = app.previous_scope_roots {
        return Some(format!(
            "showing the previous scope ({n} roots) · new roots not yet observed · press R"
        ));
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

/// A keys row from whole hints joined by ` · `, shortened to fit `width`
/// cells (counted as the main legend counts them). Hints are dropped from
/// the right, one whole hint at a time, never cut; a hint that starts with
/// `Esc` (the way out) is the last to go. A hint is never shown without its
/// label, so when not even the way out fits the row is empty.
fn fit_hints(hints: &[&str], width: usize) -> String {
    let whole = fit_hints_once(hints, width);
    if whole.contains("Esc") || !hints.iter().any(|h| h.starts_with("Esc")) {
        return whole;
    }
    // Not even the way out fits with its aside: try each hint without its
    // parenthesised aside (`Esc cancel (nothing is removed)`).
    let short: Vec<&str> = hints
        .iter()
        .map(|h| h.split(" (").next().unwrap_or(h))
        .collect();
    fit_hints_once(&short, width)
}

fn fit_hints_once(hints: &[&str], width: usize) -> String {
    let mut kept: Vec<&str> = hints.to_vec();
    loop {
        // The separator is the one fixed, well-known glyph row: 3 cells.
        let cells =
            kept.iter().map(|h| legend_cells(h)).sum::<usize>() + 3 * kept.len().saturating_sub(1);
        if cells <= width {
            return kept.join(" · ");
        }
        // Drop the last hint that is not the way out.
        match kept.iter().rposition(|h| !h.starts_with("Esc")) {
            Some(i) => {
                kept.remove(i);
            }
            None => return String::new(),
        }
    }
}

/// The key legend, shortened to fit `width` cells. Ordered by what a
/// person reaches for first: filter, view, refresh and delete come before
/// movement (arrow keys need no legend). `? help  q quit` is always kept:
/// it is how you find every other key. Items are dropped from the end.
fn footer_legend(
    width: usize,
    blocked: bool,
    markable: bool,
    filterable: bool,
    backspace_hint: Option<&str>,
) -> String {
    const BASE: [&str; 11] = [
        "Space mark",
        "Tab section",
        "v view",
        "/ filter",
        "R refresh",
        "A mark all",
        "↑↓ move",
        "→/← in/out",
        "g/s/n/t/a sort",
        "r reverse",
        "? help",
    ];
    const TAIL: &str = "q quit";
    let mut items: Vec<&str> = BASE.to_vec();
    if let Some(hint) = backspace_hint {
        items.insert(1, hint);
    }
    if !filterable {
        items.retain(|k| *k != "/ filter");
    }
    if !markable {
        // A view whose rows cannot be marked shows no key that only
        // answers with a refusal.
        items.retain(|k| !matches!(*k, "Space mark" | "A mark all"));
    }
    if blocked {
        // What the last check could not include, one key from the list.
        items.insert(items.len().min(4), "b blocked");
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
        if legend_cells(&line) <= width {
            return line;
        }
        if n == 0 {
            // Even the two fixed hints do not fit: show whole hints only,
            // never a hint cut mid-word.
            let help_quit = format!("{}  {TAIL}", all_items[all_items.len() - 1]);
            for cand in [help_quit.as_str(), TAIL] {
                if legend_cells(cand) <= width {
                    return cand.to_string();
                }
            }
            return String::new();
        }
        n -= 1;
    }
}

/// Cells a legend line may take on a terminal. Glyphs outside ASCII (the
/// `⌫` and the arrows) are East Asian ambiguous or font-dependent, so each
/// is counted as two cells: a line that fits by this count fits however the
/// terminal draws them, and a hint is dropped whole rather than cut.
fn legend_cells(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
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
            // Rows are left but not enough for the whole line. A count or
            // a reason is worth its start (it leads with the fact); a
            // warning is never cut mid-sentence, it is counted instead.
            let room = cap.saturating_sub(used + reserve);
            if room > 0 && !l.starts_with('⚠') {
                fit.push((clip_end(l, (room * width).saturating_sub(room)), *c));
                if left > 0 {
                    fit.push((more(left), Color::Yellow));
                }
            } else {
                fit.push((more(left + 1), Color::Yellow));
            }
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

fn confirm_lines(app: &App) -> Vec<(String, Color)> {
    let cargo_conflict = crate::app::selective_cargo_keep_conflict(
        app.keep_executables,
        app.marked.values().any(|u| u.cargo_unit.is_some()),
    );
    let summary = if cargo_conflict {
        app.confirm_summary().replace(
            "Enter applies this plan",
            "Enter unavailable until k turns off Keep",
        )
    } else {
        app.confirm_summary()
    };
    let mut lines: Vec<(String, Color)> = summary
        .lines()
        .map(|line| {
            let color = if line.starts_with("PERMANENT") || line.starts_with("!") {
                Color::Yellow
            } else {
                Color::Reset
            };
            (line.to_string(), color)
        })
        .collect();
    if crate::app::selective_cargo_keep_conflict(
        app.keep_executables,
        app.marked.values().any(|u| u.cargo_unit.is_some()),
    ) {
        lines.push((
            "Saved Keep executables setting conflicts with selective Cargo removal; press k to turn it off.".into(),
            Color::Yellow,
        ));
    }
    if !app.blocked.is_empty() {
        let count = app.blocked.len();
        lines.push((
            format!(
                "{count} row{} skipped; press d to review why",
                if count == 1 { "" } else { "s" }
            ),
            Color::Red,
        ));
    }
    lines
}

fn confirm_detail_lines(app: &App) -> Vec<(String, Color)> {
    let mut lines: Vec<_> = app
        .confirm_details()
        .lines()
        .map(|line| {
            (
                line.to_string(),
                if line.starts_with("  !") {
                    Color::Yellow
                } else {
                    Color::Reset
                },
            )
        })
        .collect();
    lines.push((
        "Esc returns to the summary; Enter is disabled here.".into(),
        Color::Yellow,
    ));
    lines
}

pub fn confirm_review_fingerprint(app: &App) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    confirm_lines(app).hash(&mut h);
    confirm_detail_lines(app).hash(&mut h);
    h.finish()
}

fn wrap_confirm_text(text: &str, width: usize) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;

    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    for chunk in text.split_inclusive(char::is_whitespace) {
        let candidate = format!("{current}{chunk}");
        if crate::model::display_width(&candidate) <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for grapheme in chunk.graphemes(true) {
            if !current.is_empty()
                && crate::model::display_width(&current) + crate::model::display_width(grapheme)
                    > width
            {
                lines.push(std::mem::take(&mut current));
            }
            current.push_str(grapheme);
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn confirm_visual_lines(app: &App, width: u16) -> Vec<(String, Color)> {
    let lines = if app.confirm_details_open {
        confirm_detail_lines(app)
    } else {
        confirm_lines(app)
    };
    lines
        .iter()
        .flat_map(|(line, color)| {
            wrap_confirm_text(line, usize::from(width.max(1)))
                .into_iter()
                .map(|wrapped| (wrapped, *color))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn confirm_total_rows(app: &App, width: u16) -> usize {
    confirm_visual_lines(app, width).len()
}

pub fn confirm_review_rows(app: &App, width: u16) -> usize {
    confirm_visual_lines_for(&confirm_lines(app), width).len()
}

pub fn confirm_display_rows(app: &App, width: u16) -> usize {
    confirm_total_rows(app, width)
}

fn confirm_visual_lines_for(lines: &[(String, Color)], width: u16) -> Vec<(String, Color)> {
    lines
        .iter()
        .flat_map(|(line, color)| {
            wrap_confirm_text(line, usize::from(width.max(1)))
                .into_iter()
                .map(|wrapped| (wrapped, *color))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn draw_confirm_overlay(frame: &mut Frame, app: &App, screen: Rect) {
    if screen.width < crate::app::CONFIRM_MIN_COLS || screen.height < 7 {
        return;
    }
    let content_width = screen.width.saturating_sub(2).max(1);
    let fingerprint = confirm_review_fingerprint(app);
    app.prepare_confirm_review(screen.width, screen.height, fingerprint);
    let total = confirm_total_rows(app, content_width);
    let available_height = screen.height.saturating_sub(1);
    let wanted = total.saturating_add(2).min(usize::from(available_height)) as u16;
    let area = Rect {
        x: screen.x,
        y: screen.y + (available_height.saturating_sub(wanted) / 2),
        width: screen.width,
        height: wanted,
    };
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if app.confirm_details_open {
            " All action paths · Esc summary "
        } else {
            " Review actions · l inspect paths "
        });
    let inner = block.inner(area);
    let visible = usize::from(inner.height);
    let total = confirm_total_rows(app, inner.width);
    let last = total.saturating_sub(visible);
    let first = app
        .confirm_scroll
        .get()
        .min(last)
        .min(usize::from(u16::MAX));
    app.confirm_scroll.set(first);
    app.page.set(visible.saturating_sub(1).max(1));
    if !app.confirm_details_open {
        app.note_confirm_rows_seen(first, visible, total);
    }
    let content: Vec<Line> = confirm_visual_lines(app, inner.width)
        .iter()
        .map(|(line, color)| Line::styled(line.clone(), tone(*color)))
        .collect();
    frame.render_widget(
        Paragraph::new(content)
            .block(block)
            .scroll((first.min(u16::MAX as usize) as u16, 0)),
        area,
    );
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
    rows: u16,
    title: &str,
    lines: &[(String, Color)],
    more: &dyn Fn(usize) -> String,
) {
    let h = rows.min(body.height);
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
                format!(
                    "Checked {} of {}",
                    swamp_core::render::human_count(op.completed as u64),
                    swamp_core::render::human_count(op.total as u64)
                )
            } else {
                format!(
                    "Checked {}",
                    swamp_core::render::human_count(op.completed as u64)
                )
            };
            [
                format!(
                    "{chip}  {counts} · {} ready · {} blocked",
                    swamp_core::render::human_count(op.succeeded as u64),
                    swamp_core::render::human_count(op.failed as u64)
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
                swamp_core::render::human_count(op.completed as u64),
                swamp_core::render::human_count(op.total as u64),
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

fn idle_status_rows(app: &App, rows: &[crate::model::Row], width: usize) -> [String; 2] {
    let marked = app.marked.len();
    let trash = app
        .marked
        .values()
        .filter(|unit| unit.docker.is_none())
        .count();
    let permanent = app
        .marked
        .values()
        .filter(|unit| unit.docker.is_some())
        .count();
    let selected_bytes = app
        .marked
        .values()
        .fold(0u64, |total, unit| total.saturating_add(unit.bytes));
    let count = |n: usize| swamp_core::render::human_count(n as u64);

    let first = if app.editing_filter {
        app.filter_error
            .as_ref()
            .map(|error| {
                format!(
                    "Filter not applied: {}",
                    error.strip_prefix("filter: ").unwrap_or(error)
                )
            })
            .unwrap_or_default()
    } else if marked > 0 {
        let mut parts = vec![format!("{} marked", count(marked))];
        if permanent > 0 {
            parts.push(format!("{} Docker permanent", count(permanent)));
        }
        if trash > 0 {
            parts.push(format!("{} Trash", count(trash)));
        }
        parts.push(format!("{} selected", human_bytes(selected_bytes)));
        fit_clauses(&parts, width)
    } else if !app.blocked.is_empty() {
        format!("{} blocked · b reasons", count(app.blocked.len()))
    } else {
        String::new()
    };

    let row_position = if rows.is_empty() {
        String::new()
    } else {
        format!(
            "Row {} of {}",
            count(app.selected.saturating_add(1)),
            count(rows.len())
        )
    };
    let context = rows
        .get(app.selected)
        .and_then(|row| {
            let is_marked = row
                .unit
                .as_ref()
                .is_some_and(|unit| app.marked.contains_key(&unit.0));
            if let Some(manager) = row.tool.filter(|_| !is_marked) {
                if marked > 0 {
                    Some("Space mark folder for Trash".to_string())
                } else {
                    Some(format!("Backspace opens {} list", manager.name()))
                }
            } else if marked > 0 {
                Some("Backspace review".to_string())
            } else if row.expandable {
                Some(
                    if app.view == crate::app::ViewKind::Projects
                        || row.collapsed_children.is_some()
                    {
                        "Enter open".to_string()
                    } else {
                        "Enter close".to_string()
                    },
                )
            } else {
                None
            }
        })
        .or_else(|| (marked > 0).then(|| "Backspace review".to_string()));

    let second = if app.editing_filter {
        if app.filter_error.is_some() {
            fit_clauses(&["Edit filter".into(), "Esc cancel".into()], width)
        } else {
            String::new()
        }
    } else {
        let mut parts = Vec::new();
        if marked > 0 && !app.blocked.is_empty() {
            parts.push(format!("{} blocked · b reasons", count(app.blocked.len())));
        }
        if !row_position.is_empty() {
            parts.push(row_position);
        }
        if let Some(context) = context {
            parts.push(context);
        }
        if parts.is_empty() {
            String::new()
        } else {
            fit_clauses(&parts, width)
        }
    };
    [clip_end(&first, width), clip_end(&second, width)]
}

fn draw_status(
    frame: &mut Frame,
    app: &App,
    summary: &[String],
    rows: &[crate::model::Row],
    area: Rect,
) {
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
    } else if app.help_open
        || app.picker.is_some()
        || app.tool_sheet.is_some()
        || app.cargo_inspection.is_some()
        || app.blocked_open
    {
        // Sheets and editors own their own context; idle list feedback must
        // not bleed through them.
    } else if app.editing_filter && app.filter_error.is_some() {
        let lines = idle_status_rows(app, rows, w);
        frame.render_widget(
            Paragraph::new(
                lines
                    .iter()
                    .map(|line| Line::from(line.clone()))
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().fg(Color::Red)),
            area,
        );
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
    } else {
        let lines = idle_status_rows(app, rows, w);
        frame.render_widget(
            Paragraph::new(
                lines
                    .iter()
                    .map(|line| Line::from(line.clone()))
                    .collect::<Vec<_>>(),
            ),
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
    // Chrome is the same rows in every state and every view: header, the
    // developer-storage headline block (a fixed number of rows for this
    // terminal height), filter line, two status rows, keys. Sheets
    // overlay the body; nothing resizes it.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(headline_rows(size.height)),
            Constraint::Length(1), // view strip
            Constraint::Length(1), // filter line
            Constraint::Min(1),    // body
            Constraint::Length(STATUS_ROWS),
            Constraint::Length(1), // keys
        ])
        .split(size);

    draw_header(frame, app, chunks[0]);
    draw_headline(frame, app, chunks[1]);
    draw_view_strip(frame, app, chunks[2]);
    draw_filter_line(frame, app, chunks[3]);
    let rows = app.rows();
    draw_body(frame, app, chunks[4], &rows);
    if app.operation.is_none() && app.blocked_open {
        // Two rows per item.
        app.page
            .set((usize::from(SHEET_ROWS.min(chunks[4].height)).saturating_sub(2) / 2).max(1));
        draw_sheet(
            frame,
            chunks[4],
            SHEET_ROWS,
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
    }
    draw_status(frame, app, &summary, &rows, chunks[5]);
    if app.confirm_open {
        draw_confirm_overlay(frame, app, size);
        if app.blocked_open {
            app.page
                .set((usize::from(SHEET_ROWS.min(chunks[4].height)).saturating_sub(2) / 2).max(1));
            let title = if app.blocked_scroll > 0 {
                format!(
                    "Blocked: {} · from item {} · Esc returns to plan",
                    app.blocked.len(),
                    app.blocked_scroll + 1
                )
            } else {
                format!("Blocked: {} · Esc returns to plan", app.blocked.len())
            };
            draw_sheet(
                frame,
                chunks[4],
                SHEET_ROWS,
                &title,
                &blocked_sheet(app),
                &|_| "more below (↓ to scroll)".to_string(),
            );
        }
    }

    // The keys row is the legend for the state you are actually in, and
    // nothing else ever replaces it.
    let fw = size.width as usize;
    let footer_text = if let Some(op) = &app.operation {
        match op.label {
            "Reviewing" => fit_hints(
                &[
                    "Esc cancel",
                    "Nothing has been changed",
                    "Next: review, then Enter to move to Trash",
                ],
                fw,
            ),
            "Deleting" => fit_hints(
                &["Esc stop after this item", "moved items stay in Trash"],
                fw,
            ),
            _ => fit_hints(&["Esc cancel"], fw),
        }
    } else if app.blocked_open {
        if app.confirm_open {
            fit_hints(&["↑↓ scroll", "r check again", "Esc back to the plan"], fw)
        } else {
            fit_hints(&["↑↓ scroll", "r check again", "Esc close"], fw)
        }
    } else if app.confirm_open {
        let complete = app.confirm_review_is_complete(size.width, size.height);
        let trash_n = app.marked.values().filter(|u| u.docker.is_none()).count();
        let docker_n = app.marked.values().filter(|u| u.docker.is_some()).count();
        let action = match (trash_n > 0, docker_n > 0) {
            (true, true) => "Enter apply actions",
            (true, false) => "Enter move to Trash",
            (false, true) => "Enter remove permanently",
            (false, false) => "Esc cancel",
        };
        let mut clauses: Vec<&str> = if app.marked.is_empty() {
            vec!["No actions marked", "Esc cancel"]
        } else if app.blocked_open {
            vec!["Esc return to plan", "↑↓ scroll"]
        } else if app.confirm_details_open {
            vec!["Esc summary", "↑↓ PgUp PgDn scroll", "Enter disabled"]
        } else if crate::app::selective_cargo_keep_conflict(
            app.keep_executables,
            app.marked.values().any(|u| u.cargo_unit.is_some()),
        ) {
            vec![
                "k turn off saved Keep setting",
                "Enter unavailable",
                "Esc cancel",
            ]
        } else if !app.confirm_fits(size.width, size.height) {
            vec!["Terminal too small to review", "Esc cancel"]
        } else if complete {
            vec![action, "↑↓ PgUp PgDn Home End review", "Esc cancel"]
        } else if confirm_review_rows(app, size.width.saturating_sub(2)) > usize::from(u16::MAX) {
            vec!["Plan exceeds scroll limit; mark fewer items", "Esc cancel"]
        } else {
            vec![
                "Read every line before action",
                "↑↓ PgUp PgDn Home End",
                "Esc cancel",
            ]
        };
        if !app.blocked.is_empty() {
            clauses.push("d blocked");
        }
        if !app.confirm_details_open && app.confirm_fits(size.width, size.height) {
            clauses.insert(1, "l inspect paths");
        }
        // Selective Cargo groups cannot use the executable-copy mode.
        if !app.marked.values().all(|u| u.reclaim.is_some())
            && !app.marked.values().any(|u| u.cargo_unit.is_some())
        {
            clauses.push(if app.keep_executables {
                "keep executables → bin/ (k)"
            } else {
                "k keep executables"
            });
        }
        fit_hints(&clauses, fw)
    } else if app.picker.is_some() {
        fit_hints(
            &[
                "↑↓ field",
                "←→ value",
                "Enter apply",
                "Esc cancel",
                "e edit as text",
                "0 clear",
            ],
            fw,
        )
    } else if app.editing_filter {
        fit_hints(&["Tab complete", "Enter apply", "Esc cancel"], fw)
    } else {
        let markable = !rows.is_empty()
            && match app.view {
                crate::app::ViewKind::Reclaim
                | crate::app::ViewKind::Disk
                | crate::app::ViewKind::DiskGaps => {
                    rows.get(app.selected).is_some_and(|row| row.unit.is_some())
                }
                crate::app::ViewKind::Kinds | crate::app::ViewKind::Types => false,
                _ => true,
            };
        let backspace_hint = match rows.get(app.selected) {
            Some(row) => {
                let is_marked = row
                    .unit
                    .as_ref()
                    .is_some_and(|unit| app.marked.contains_key(&unit.0));
                match row.tool {
                    Some(manager) if !is_marked && app.marked.is_empty() => {
                        Some(format!("⌫ {} list", manager.name()))
                    }
                    Some(_) if !is_marked => None,
                    _ if markable || !app.marked.is_empty() => Some("⌫ review".to_string()),
                    _ => None,
                }
            }
            None if !app.marked.is_empty() => Some("⌫ review".to_string()),
            None => None,
        };
        footer_legend(
            size.width as usize,
            !app.blocked.is_empty(),
            markable,
            app.view.uses_filter(),
            backspace_hint.as_deref(),
        )
    };
    frame.render_widget(Paragraph::new(footer_text), chunks[6]);

    if app.help_open {
        draw_help(frame, app, size);
    }
    if let Some(p) = &app.picker {
        draw_picker(frame, app, p, size);
    }
    if let Some(sheet) = &app.tool_sheet {
        draw_tool_sheet(frame, sheet, size);
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

/// Fixed height across views and states: total plus coverage, or just the
/// total on terminals shorter than sixteen rows. Leave the rest for the table.
pub fn headline_rows(height: u16) -> u16 {
    if height >= 16 {
        2
    } else if height >= 12 {
        1
    } else {
        0
    }
}

/// Clauses that each come in shorter forms (longest first), in priority
/// order. Every clause steps to the same form at once (longest, then the
/// next, and so on); at each form as many leading clauses as fit are kept.
/// The form that keeps the most clauses wins, and among equals the one
/// with the fullest wording. So trailing clauses give way before leading
/// clauses lose words. When not even the first clause fits, its shortest
/// form is cut to the width.
fn fit_tiered(clauses: &[Vec<String>], width: usize) -> String {
    let clauses: Vec<&Vec<String>> = clauses.iter().filter(|c| !c.is_empty()).collect();
    let levels = clauses.iter().map(|c| c.len()).max().unwrap_or(0);
    let mut best: Option<(usize, usize, String)> = None;
    for level in 0..levels {
        let picked: Vec<String> = clauses
            .iter()
            .map(|c| c[level.min(c.len() - 1)].clone())
            .collect();
        let mut kept = 0;
        let mut line = String::new();
        for k in 1..=picked.len() {
            let candidate = picked[..k].join(" · ");
            if crate::model::display_width(&candidate) > width {
                break;
            }
            kept = k;
            line = candidate;
        }
        if kept > best.as_ref().map_or(0, |b| b.0) {
            best = Some((kept, level, line));
        }
    }
    match best {
        Some((_, _, line)) => line,
        None => {
            let shortest: Vec<String> = clauses.iter().map(|c| c[c.len() - 1].clone()).collect();
            fit_clauses(&shortest, width)
        }
    }
}

/// One total and one coverage line. Detailed allocation lives in Disk.
pub fn headline_lines(app: &App, width: usize, now: u64) -> [String; 2] {
    use swamp_core::headline::Disk;
    if !app.has_index {
        return [
            clip_end("Developer storage: not measured yet", width),
            clip_end("Press R to scan; saved observations appear here.", width),
        ];
    }
    let h = app.headline();
    let mut coverage: Vec<Vec<String>> = Vec::new();
    // A discrepancy changes how every number should be read, so it wins.
    if h.audit_sentence().is_some() {
        coverage.push(if width >= 64 {
            vec!["FLAG: walk spot audit disagrees: see swamp report --view disk".into()]
        } else {
            vec!["FLAG: spot audit disagrees (3 Disk)".into()]
        });
    }
    if let Some(n) = h.previous_scope_roots {
        coverage.push(vec![
            format!("previous scope ({n} roots) · R refresh"),
            "previous scope · R refresh".into(),
        ]);
    }
    if h.scope == "explicit_root" {
        coverage.push(vec![
            "only the root named on the command line".into(),
            "one root only".into(),
        ]);
    }
    match &h.disk {
        Disk::Measured(m) => {
            if m.percent_of_used_tenths.is_none() && m.exceeds_used {
                coverage.push(vec![
                    "FLAG: more than the disk's used bytes; no percent".into(),
                    "FLAG: over used; no percent".into(),
                ]);
            } else if m.percent_of_used_tenths.is_none() && h.scope == "current" {
                coverage.push(if m.container_used == Some(0) {
                    vec![
                        "disk ledger: used bytes are zero; no percent".into(),
                        "ledger: zero used; no percent".into(),
                    ]
                } else {
                    vec![
                        "disk ledger: no used figure; no percent".into(),
                        "ledger: no used figure".into(),
                    ]
                });
            }
            if m.not_measured.directories > 0 {
                let count = swamp_core::render::human_count(m.not_measured.directories as u64);
                coverage.push(vec![
                    format!(
                        "Not measured: {count} folder{}",
                        if m.not_measured.directories == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ),
                    format!(
                        "{count} folder{} not measured",
                        if m.not_measured.directories == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ),
                ]);
            }
            let age = swamp_core::volume_ledger::age_text(m.measured_at, now);
            let older = if m.older_than_observation_secs.is_some() {
                " (older than scan)"
            } else {
                ""
            };
            coverage.push(vec![
                format!("disk ledger measured {age}{older}"),
                format!("ledger {age}{older}"),
            ]);
        }
        Disk::NotMeasured => coverage.push(vec![
            "disk ledger: not measured yet · 3 Disk for details".into(),
            "disk ledger: not measured yet".into(),
        ]),
        Disk::Unreadable { .. } => coverage.push(vec![
            "disk ledger: could not be read · 3 Disk for details".into(),
            "disk ledger: unreadable".into(),
        ]),
        Disk::Newer { .. } => coverage.push(vec![
            "disk ledger: written by a newer swamp; not used".into(),
            "disk ledger: newer swamp; not used".into(),
        ]),
        Disk::FutureDated { .. } => coverage.push(vec![
            "disk ledger: dated in the future; not used".into(),
            "ledger: future date; not used".into(),
        ]),
    }
    [
        fit_tiered(&[h.line_tiers()], width),
        fit_tiered(&coverage, width),
    ]
}

fn draw_headline(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let all = headline_lines(app, area.width as usize, swamp_core::entities::now());
    let lines: Vec<&String> = if area.height == 1 && all[1].starts_with("FLAG:") {
        vec![&all[1]]
    } else {
        all.iter().take(area.height as usize).collect()
    };
    let dim = Style::default().add_modifier(Modifier::DIM);
    let rows: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            // The headline is the one bold line; the rest stays quiet.
            let style = if i == 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            Line::from(Span::styled((*l).clone(), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), area);
}

/// The tool-managed removal sheet (#177): a fixed-layout popup whose
/// last inner row is always its keys.
fn draw_tool_sheet(frame: &mut Frame, sheet: &crate::tool_sheet::ToolSheet, size: Rect) {
    let popup = Rect {
        x: size.x + 1,
        y: size.y + 1,
        width: size.width.saturating_sub(2),
        height: size.height.saturating_sub(2),
    };
    if popup.width < 4 || popup.height < 3 {
        return;
    }
    // The sheet swallows every key but its own, so nothing of the screen
    // beneath (its rail, its footer key hints) may show around it.
    frame.render_widget(Clear, size);
    frame.render_widget(Clear, popup);
    sheet.note_drawn(size.width, size.height);
    let (width, rows) = crate::tool_sheet::body_size(size.width, size.height);
    let mut lines: Vec<Line> = sheet
        .body(width, rows)
        .into_iter()
        .map(Line::from)
        .collect();
    while lines.len() < rows {
        lines.push(Line::from(""));
    }
    let keys = fit_hints(&sheet.key_hints(), width);
    lines.push(
        Line::from(keys).style(Style::default().add_modifier(ratatui::style::Modifier::REVERSED)),
    );
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(sheet.title())),
        popup,
    );
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

/// The section strip: one row naming the three sections, the current one
/// in reverse video (an attribute, not a color: it survives `NO_COLOR` and
/// any theme). `1 Projects  2 Tools  3 Disk` while the digits fit, then the
/// bare names; `· Tab section` follows when there is room. The sub-view is
/// named on the line below (`view: Tools › Reclaim (1 of 3 · v next)`).
pub fn view_strip_spans(app: &App, width: usize) -> Vec<Span<'static>> {
    use crate::app::Section;
    let dw = crate::model::display_width;
    let cur = app.view.section();
    let build = |digits: bool| -> Vec<String> {
        Section::ALL
            .iter()
            .map(|s| {
                if digits {
                    format!("{} {}", s.key(), s.title())
                } else {
                    s.title().to_string()
                }
            })
            .collect()
    };
    let total = |l: &[String]| l.iter().map(|x| dw(x)).sum::<usize>() + 2 * (l.len() - 1);
    let labels = if total(&build(true)) <= width {
        build(true)
    } else if total(&build(false)) <= width {
        build(false)
    } else {
        // Too narrow for all three: the current section alone, cut to fit.
        let name = cur.title().to_string();
        vec![clip_end(&name, width.max(1))]
    };
    let hint = if app.views_seen {
        ""
    } else {
        " · Tab switches sections"
    };
    let quiet = Style::default().add_modifier(Modifier::DIM);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let l = clip_end(label, width.max(1));
        spans.push(if labels.len() == 1 || Section::ALL[i] == cur {
            Span::styled(l, selected_style())
        } else {
            Span::raw(l)
        });
    }
    if total(&labels) + dw(hint) <= width {
        spans.push(Span::styled(hint.to_string(), quiet));
    }
    spans
}

fn draw_view_strip(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(view_strip_spans(app, area.width as usize))),
        area,
    );
}

/// `filter: growth > 100MB in 7d · sort: size ↑`: what narrows and orders
/// the rows.
fn filter_clause(app: &App, width: usize) -> String {
    use crate::app::ViewKind as V;
    use swamp_core::filter::Predicate;
    let title = if app.view == V::Tree {
        app.selected_project
            .as_ref()
            .map(|p| format!("{} › {p}", app.view.title()))
            .unwrap_or_else(|| app.view.title().into())
    } else {
        app.view.title().to_string()
    };
    let filter = if matches!(app.view, V::Kinds | V::Types) {
        let relevant: Vec<_> = app
            .filter
            .predicates
            .iter()
            .filter_map(|p| match (app.view, p) {
                (V::Kinds, Predicate::Kind(k)) => Some(format!("kind:{k}")),
                (V::Types, Predicate::Type(t)) => Some(format!("type:{t}")),
                _ => None,
            })
            .collect();
        let others = relevant.len() < app.filter.predicates.len();
        let value = if relevant.is_empty() {
            "all".into()
        } else {
            relevant.join(" ")
        };
        format!(
            "filter: {value}{}",
            if others { " (other filters off)" } else { "" }
        )
    } else if app.view.uses_filter() {
        let ignored: Vec<&str> = app
            .filter
            .predicates
            .iter()
            .filter_map(|p| match (app.view, p) {
                (V::Projects | V::Builds | V::Deps, Predicate::Kind(_)) => Some("kind"),
                (V::Tree, Predicate::Project(_)) => Some("project"),
                (V::Tree, Predicate::Type(_)) => Some("type"),
                (V::Builds | V::Deps, Predicate::IdleGreaterThan(_)) => Some("idle"),
                (V::Builds | V::Deps, Predicate::MergeComplete) => Some("merge-complete"),
                (V::Builds | V::Deps, Predicate::Pr(_)) => Some("pr"),
                _ => None,
            })
            .collect();
        let value = if app.filter_text.trim().is_empty() || app.filter_text == "0" {
            "none"
        } else {
            &app.filter_text
        };
        if ignored.is_empty() {
            format!("filter: {value}")
        } else {
            format!("filter: ({} off) {value}", ignored.join(", "))
        }
    } else {
        String::new()
    };
    let sort = if matches!(app.view, V::Tree | V::Reclaim | V::Disk | V::DiskGaps) {
        String::new()
    } else {
        let name = match app.sort {
            crate::model::Sort::Growth => "growth",
            crate::model::Sort::Size => "size",
            crate::model::Sort::Name => "name",
            crate::model::Sort::Type => "ecosystem",
            crate::model::Sort::Age => "age",
            crate::model::Sort::None => "",
        };
        if name.is_empty() {
            String::new()
        } else {
            format!("sort: {name}{}", if app.reverse { " reversed" } else { "" })
        }
    };
    // The current view always has a name. Only settings used by this view
    // are shown; purpose fills spare space instead of competing with them.
    if !filter.is_empty() && crate::model::display_width(&format!("{title} · {filter}")) > width {
        if width < 40 {
            return clip_end(&title, width);
        }
        let short_title = clip_end(&title, width.saturating_sub(14).max(1));
        return clip_end(&format!("{short_title} · {filter}"), width);
    }
    fit_clauses(
        &[
            title,
            filter,
            sort,
            if app.view == V::Tree {
                "Esc: projects".into()
            } else {
                String::new()
            },
            app.view.purpose().into(),
        ],
        width,
    )
}

fn draw_filter_line(frame: &mut Frame, app: &App, area: Rect) {
    let line = if app.editing_filter {
        use unicode_segmentation::UnicodeSegmentation;

        let width = area.width as usize;
        let prefix = "filter › ";
        let room = width.saturating_sub(crate::model::display_width(prefix) + 1);
        let draft = if crate::model::display_width(&app.filter_text) <= room {
            app.filter_text.clone()
        } else if room == 0 {
            String::new()
        } else {
            let mut used = 1; // Leading ellipsis names the hidden start.
            let tail: Vec<&str> = app
                .filter_text
                .graphemes(true)
                .rev()
                .take_while(|g| {
                    used += crate::model::display_width(g);
                    used <= room
                })
                .collect();
            format!("…{}", tail.into_iter().rev().collect::<String>())
        };
        let input = if width <= crate::model::display_width(prefix) {
            "▏".to_string()
        } else {
            format!("{prefix}{draft}▏")
        };
        // The footer owns editing keys. Completion candidates use spare
        // width only; the current insertion point never leaves the screen.
        let completions = if app.completions.is_empty() {
            String::new()
        } else {
            format!("Tab: {}", app.completions.join("  "))
        };
        Line::from(fit_clauses(&[input, completions], width))
    } else {
        Line::from(filter_clause(app, area.width as usize))
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// What an empty list says: why it is empty and what to press next.
fn empty_state(app: &App) -> String {
    use crate::app::ViewKind as V;
    use swamp_core::filter::Predicate as P;
    if !app.has_index && !matches!(app.view, V::Disk | V::DiskGaps) {
        return if app.store_rebuild {
            "Rebuilding observations from an older store in the background. Settings, protections and notes are kept; growth history starts again. Press R to scan.".into()
        } else {
            "No saved observations yet. Press R to scan in the background.".into()
        };
    }
    let has_active_filter = app.filter.predicates.iter().any(|p| match app.view {
        V::Projects => !matches!(p, P::Kind(_)),
        V::Tree => !matches!(p, P::Project(_) | P::Type(_)),
        V::Builds | V::Deps => matches!(
            p,
            P::Project(_) | P::Type(_) | P::Size { .. } | P::Growth { .. } | P::AgeGreaterThan(_)
        ),
        V::Kinds => matches!(p, P::Kind(_)),
        V::Types => matches!(p, P::Type(_)),
        _ => false,
    });
    if has_active_filter {
        return "No matches for the active filters. Press / to edit, or 0 to clear.".into();
    }
    match app.view {
        V::Projects => format!(
            "No projects recorded under {}. Press R to scan again.",
            app.root.display()
        ),
        V::Tree => "No folders recorded for this project. Press Esc for Projects.".into(),
        V::Builds => {
            "No build outputs recorded. Press v for another view, or R to scan again.".into()
        }
        V::Deps => {
            "No dependency folders recorded. Press v for another view, or R to scan again.".into()
        }
        V::Kinds => {
            "No storage categories recorded. Press v for another view, or R to scan again.".into()
        }
        V::Types => {
            "No project ecosystems recorded. Press v for another view, or R to scan again.".into()
        }
        V::Unowned => "No unassigned storage recorded. Press v for another view.".into(),
        V::Docker => {
            "No Docker storage recorded. Press v for another view, or R to refresh observations."
                .into()
        }
        V::External => {
            "No tool storage recorded. Press v for another view, or R to scan again.".into()
        }
        V::Agents => {
            "No agent storage recorded. Press v for another view, or R to scan again.".into()
        }
        V::Reclaim => format!(
            "No storage units recorded. {}. Press v for another view, or R to scan again.",
            app.reclaim_view().scope.statement
        ),
        V::Disk | V::DiskGaps => {
            let h = app.headline();
            if matches!(h.disk, swamp_core::headline::Disk::Measured(_)) {
                "No coverage gaps recorded in the disk ledger. Press v for Disk usage.".into()
            } else {
                let why = h
                    .disk_state_sentence()
                    .unwrap_or_else(|| "No disk ledger recorded".into());
                let next = if why.contains("swamp observe --volume") {
                    ""
                } else {
                    " Run `swamp observe --volume` to measure the disk."
                };
                format!(
                    "{why}.{next} Scheduled scans measure the disk daily. Opening this view does not scan."
                )
            }
        }
    }
}

fn draw_body(frame: &mut Frame, app: &App, area: Rect, rows: &[crate::model::Row]) {
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
    // The Reclaim view says, under its heading, what its consumer evidence
    // was checked against (and how many coverage notes `swamp report
    // --view reclaim` prints): one line, the same on every screen size.
    let reclaim_line = (app.view == crate::app::ViewKind::Reclaim).then(|| {
        let view = app.reclaim_view();
        let notes = view.coverage_notes.len();
        let full = if notes == 0 {
            view.scope.statement.clone()
        } else {
            format!(
                "{} · {notes} coverage notes in swamp report --view reclaim",
                view.scope.statement
            )
        };
        if crate::model::display_width(&full) <= width {
            full
        } else {
            let scope = if view.scope.declared_roots > 0 {
                format!(
                    "{} declared root{}{}",
                    view.scope.declared_roots,
                    if view.scope.declared_roots == 1 {
                        ""
                    } else {
                        "s"
                    },
                    if view.scope.complete {
                        ""
                    } else {
                        "; incomplete"
                    }
                )
            } else if view
                .scope
                .incomplete_because
                .iter()
                .any(|r| r == "no source roots are declared")
            {
                "default roots only".into()
            } else {
                "incomplete scope".into()
            };
            format!(
                "{} projects in {scope} · swamp report --view reclaim",
                swamp_core::render::human_count(view.scope.projects as u64)
            )
        }
    });
    use crate::app::ViewKind as V;
    let disk_view = matches!(app.view, V::Disk | V::DiskGaps);
    let decision_view = cleanup_view || app.view == V::Reclaim || disk_view;
    let show_growth = !disk_view && (!decision_view || width >= 100);
    let half: usize = if width >= 140 && !decision_view { 6 } else { 0 };
    // name | bytes(10) | sp | growth(10) | sp | half│half | sp | signals
    let bar_width = if half > 0 { half * 2 + 2 } else { 0 };
    let fixed = if show_growth { 24 + bar_width } else { 13 };
    let flexible = width.saturating_sub(fixed);
    let signals_width = if disk_view {
        flexible * 2 / 5
    } else if decision_view {
        (flexible / 2).min(64)
    } else if width >= 100 {
        flexible / 3
    } else {
        0
    };
    let name_width = if decision_view {
        flexible.saturating_sub(signals_width).min(64)
    } else {
        flexible.saturating_sub(signals_width)
    };
    let signals_width = if decision_view {
        flexible.saturating_sub(name_width)
    } else {
        signals_width
    };
    let heading = format!(
        "{}{:>10} {}{}{}",
        pad_display(
            match app.view {
                V::Projects => "Project",
                V::Tree => "Folder",
                V::Builds => "Build output",
                V::Deps => "Dependencies",
                V::Types => "Ecosystem",
                V::Kinds => "Storage kind",
                V::Unowned => "Unassigned storage",
                V::Reclaim => "Storage item",
                V::Docker => "Docker object",
                V::External => "Tool location",
                V::Agents => "Agent storage",
                V::Disk => "Disk allocation",
                V::DiskGaps => "Location",
            },
            name_width
        ),
        if cleanup_view { "Size*" } else { "Size" },
        if show_growth {
            format!("{:>10} ", "Change")
        } else {
            String::new()
        },
        pad_display(if half > 0 { "Change bar" } else { "" }, bar_width),
        pad_display(
            if cleanup_view || reclaim_line.is_some() {
                "If removed"
            } else if disk_view {
                "Measurement"
            } else {
                "Details"
            },
            signals_width
        )
    );
    let mut lines: Vec<Line> = vec![Line::styled(
        heading,
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if cleanup_view || rows.iter().any(|r| r.allocated) {
        lines.push(Line::raw(if cleanup_view {
            "* allocated bytes; shared files may be counted again. Age = modified"
        } else {
            "* allocated bytes; shared files may be counted again"
        }));
    }
    if let Some(line) = &reclaim_line {
        lines.push(Line::styled(
            clip_end(line, width),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    // The detail pane is the same height whichever row is selected, so
    // the table never resizes under the cursor.
    let detail_height = DETAIL_ROWS.min(area.height / 3);
    let table_height = area.height.saturating_sub(detail_height);
    let header_count = lines.len() as u16;
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
    // Only visible rows need terminal-cell formatting and mark decoration.
    // The full row set above still owns ordering, totals and growth-bar scale.
    let mark_states = app.project_mark_states();
    for (i, row) in rows.iter().enumerate().skip(offset).take(visible) {
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
            match &row.size_text {
                Some(text) => clip_end(text, 10),
                None => format!(
                    "{}{}",
                    human_bytes(row.bytes),
                    if row.allocated { "*" } else { "" }
                ),
            }
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
            .filter(|n| *n > 0)
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
        } else if app.view == V::Reclaim {
            row.signals[0].clone()
        } else if narrow {
            pick_signals(&row.signals, 2).join(" · ")
        } else {
            row.signals.join(" · ")
        };
        if row.cleanup_summary.is_none() && signals_text.chars().count() > signals_width {
            // Never overflow the row: prefer the loud signals, then cut.
            if app.view != V::Reclaim {
                signals_text = pick_signals(&row.signals, 3).join(" · ");
            }
            if signals_text.chars().count() > signals_width {
                signals_text = clip_end(&signals_text, signals_width);
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
                        // Keep the consequence intact; an expand glyph already
                        // lives in the name, so the child count gives way first.
                        if crate::model::display_width(&format!("{signals_text}{hidden}"))
                            <= signals_width
                        {
                            format!("{signals_text}{hidden}")
                        } else {
                            signals_text
                        }
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
    let shown_count = lines.len() as u16;
    frame.render_widget(
        Paragraph::new(lines),
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
    heading(&mut out, "Common tasks");
    entry(&mut out, "1  2  3", "Projects · Tools and Reclaim · Disk");
    entry(
        &mut out,
        "→ / Enter",
        "inspect the selected project or folder",
    );
    entry(&mut out, "s / g", "sort by size / growth");
    entry(&mut out, "/ / 0", "filter the list / clear the filter");
    entry(
        &mut out,
        "Space",
        "mark items for review; nothing changes yet",
    );
    entry(
        &mut out,
        "Backspace",
        "review paths, costs and warnings before removal",
    );
    entry(&mut out, "Esc", "cancel or go back");
    entry(&mut out, "R", "refresh stored measurements");
    plain(&mut out, "Protect a path: swamp protect add <path>", 0);
    blank(&mut out);
    heading(&mut out, "Full key reference");
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
    entry(
        &mut out,
        "A",
        "mark every row here that swamp has a cleanup rule for. Rows it keeps by default or has no rule for are marked one at a time with Space; in Reclaim and External each unit is marked once",
    );
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
        "",
        "Reclaim, External and Disk rows, and the folders listed under them: Space marks the real folder, Backspace opens a confirm with its exact path, size and what swamp does not know (last used, regeneration cost, who has it open), and Trash is the way back. Refused only for a path that is not a real folder or file, an OS refusal, a mark that changed since you made it, an unwritable ledger, an overlap, your own protect mark, or swamp's own ledger or Trash (or a folder holding them).",
    );
    entry(
        &mut out,
        "",
        "mise installs and simulator runtimes: Backspace on an unmarked row opens the manager's own list and dry run, then its command, permanently: no Trash. Space marks the folder for Trash instead.",
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
        "Tab",
        "next section (Projects, Tools, Disk); Shift-Tab goes back. In the : filter editor Tab completes as before; the / picker ignores it",
    );
    entry(
        &mut out,
        "v",
        "next view inside the current section, wrapping around; Esc returns to projects",
    );
    entry(
        &mut out,
        "1 2 3",
        "jump to a section: 1 Projects, 2 Tools, 3 Disk",
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
    heading(
        &mut out,
        "Sections and views (Tab, v and 1 2 3 reach every one)",
    );
    for sec in crate::app::Section::ALL {
        entry(
            &mut out,
            &format!("{} {}", sec.key(), sec.title()),
            sec.describe(),
        );
        for (i, v) in sec.views().iter().enumerate() {
            entry(
                &mut out,
                "",
                &format!("{}. {}: {}", i + 1, v.title(), v.describe()),
            );
        }
    }
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
    if !app.declared_lines.is_empty() {
        blank(&mut out);
        heading(&mut out, "Declared source roots");
        for l in &app.declared_lines {
            plain(&mut out, l, 4);
        }
        plain(
            &mut out,
            "  swamp config add-root <path> declares one; the state is from the last observation",
            4,
        );
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
    let range = format!(
        "{}-{} of {}",
        (top + 1).min(lines.len()),
        (top + inner_h).min(lines.len()),
        lines.len()
    );
    let title = format!(
        " {} ",
        fit_hints(
            &["help", "↑↓ PgUp PgDn Home End scroll", "Esc closes", &range],
            usize::from(popup.width.saturating_sub(4)),
        )
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

#[cfg(test)]
mod confirm_wrap_tests {
    use super::wrap_confirm_text;

    #[test]
    fn confirmation_wrapping_preserves_path_whitespace_and_unicode_graphemes() {
        let path = "/my  cache/模型/e\u{301}/file";
        let wrapped = wrap_confirm_text(path, 5);
        assert_eq!(wrapped.concat(), path);
        assert!(
            wrapped
                .iter()
                .all(|line| crate::model::display_width(line) <= 5)
        );
        assert!(wrapped.iter().any(|line| line.contains("  ")));
        assert!(wrapped.iter().any(|line| line.contains("e\u{301}")));

        let prose = wrap_confirm_text("space is freed", 10);
        assert_eq!(prose, ["space is ", "freed"]);
        assert_eq!(prose.concat(), "space is freed");
    }
}
