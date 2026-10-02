//! The tool-managed removal sheet (#177): Backspace on a mise installs or
//! simulator runtimes row opens it. It lists what the manager itself
//! lists, reviews one item (the manager's own dry run, swamp's refusals),
//! shows the confirm, and on Y runs exactly the command shown. Every
//! step that touches the machine runs on a worker (`worker::spawn`, in
//! `app.rs`); this module only holds the state and lays out its lines.
//!
//! Layout rules: the lines are in a fixed order, each cut to the width
//! (the command wraps, never cut), and the keys are always the last row,
//! so nothing moves between frames and nothing important scrolls away.

use crate::model::{display_width, human_bytes, truncate_middle};
use swamp_core::tool_removal::{Listing, Manager, Outcome, Preview, Refusal};

/// Where the sheet is.
#[derive(Debug, Clone)]
pub enum Stage {
    /// The manager's listing is being read.
    Listing,
    /// The listing is shown; the human picks one.
    Choose,
    /// The review (dry run, refusals, open files) is running for this item.
    Reviewing(String),
    /// The confirm: Y runs exactly this preview's command.
    Confirm(Box<Preview>),
    /// swamp will not run it: the reason and the next step.
    Refused { what: String, refusal: Refusal },
    /// The removal is running (it is not stopped from here).
    Running(String),
    /// What happened, honestly.
    Done(Box<Outcome>),
}

/// The sheet's state.
#[derive(Debug, Clone)]
pub struct ToolSheet {
    pub manager: Manager,
    pub stage: Stage,
    pub listing: Option<Listing>,
    pub cursor: usize,
    /// When the confirm was first drawn (set by the drawing code, which
    /// only has `&self`): `Y` counts from there.
    /// When the confirm was first painted in full, and at which terminal
    /// size (set by the drawing code, which only has `&self`).
    first_drawn: std::cell::Cell<Option<(std::time::Instant, u16, u16)>>,
}

/// How long a confirm must have been on screen before `Y` runs it.
pub const HOLD_OFF: std::time::Duration = std::time::Duration::from_millis(1000);

/// What a tool worker reports.
#[derive(Debug)]
pub enum ToolEvent {
    Listed(Result<Listing, Refusal>),
    Reviewed(Result<Box<Preview>, Refusal>),
    Ran(Box<Outcome>),
}

impl ToolSheet {
    pub fn new(manager: Manager) -> Self {
        ToolSheet {
            manager,
            stage: Stage::Listing,
            listing: None,
            cursor: 0,
            first_drawn: std::cell::Cell::new(None),
        }
    }

    /// The drawing code calls this each time it paints the sheet at
    /// `w`x`h`. The hold-off starts at the first paint where the whole
    /// confirm fits, restarts when the size changes, and is cleared by a
    /// paint where it does not fit.
    pub fn note_drawn(&self, w: u16, h: u16) {
        let Stage::Confirm(p) = &self.stage else {
            return;
        };
        if !confirm_fits(p, w, h) {
            self.first_drawn.set(None);
            return;
        }
        match self.first_drawn.get() {
            Some((_, dw, dh)) if (dw, dh) == (w, h) => {}
            _ => self
                .first_drawn
                .set(Some((std::time::Instant::now(), w, h))),
        }
    }

    /// Forgets when a confirm was drawn (a new confirm starts over).
    pub fn disarm(&mut self) {
        self.first_drawn.set(None);
    }

    /// Whether the whole confirm has been on screen at exactly `w`x`h`
    /// for at least [`HOLD_OFF`].
    pub fn armed(&self, w: u16, h: u16) -> bool {
        matches!(self.stage, Stage::Confirm(_))
            && self
                .first_drawn
                .get()
                .is_some_and(|(t, dw, dh)| (dw, dh) == (w, h) && t.elapsed() >= HOLD_OFF)
    }

    /// Whether a worker is running for this sheet.
    pub fn waiting(&self) -> bool {
        matches!(
            self.stage,
            Stage::Listing | Stage::Reviewing(_) | Stage::Running(_)
        )
    }

    /// The border title.
    pub fn title(&self) -> String {
        format!(" Remove with {}, no Trash ", self.manager.name())
    }

    /// The keys for this stage as whole hints: always the sheet's last
    /// row. The way out comes first so it is the last to be dropped.
    pub fn key_hints(&self) -> Vec<&'static str> {
        match self.stage {
            Stage::Listing | Stage::Reviewing(_) => vec!["Esc cancel (nothing is removed)"],
            Stage::Choose => vec!["↑↓ choose", "Enter review", "Esc close"],
            Stage::Confirm(_) => vec!["Y remove (cannot be undone)", "Esc cancel"],
            Stage::Refused { .. } => vec!["Esc back"],
            Stage::Running(_) => vec!["Running: keys wait until the manager finishes"],
            Stage::Done(_) => vec!["Esc close", "R refresh swamp's measurements"],
        }
    }

    /// The body, at most `height` lines each at most `width` cells wide,
    /// in a fixed order for the stage.
    pub fn body(&self, width: usize, height: usize) -> Vec<String> {
        let name = self.manager.name();
        let mut out: Vec<String> = Vec::new();
        match &self.stage {
            Stage::Listing => out.push(format!("Reading {name}'s own list. Nothing is removed.")),
            Stage::Reviewing(what) => {
                out.push(format!(
                    "Reviewing {what}: {name}'s list, its own dry run, open files."
                ));
                out.push("Nothing is removed until you confirm.".into());
            }
            Stage::Running(what) => {
                out.push(format!("Removing {what} with {name}."));
                out.push(
                    "It is not stopped from here: a manager stopped halfway can leave a \
                     half-removed install."
                        .into(),
                );
            }
            Stage::Choose => return self.choose_lines(width, height),
            Stage::Confirm(p) => return confirm_lines(p, width, height),
            Stage::Refused { what, refusal } => {
                out.push(format!(
                    "Swamp will not run this removal ({what}). Nothing ran."
                ));
                push_wrapped(&mut out, &format!("Reason: {}", refusal.reason), width, 3);
                push_wrapped(&mut out, &format!("Next: {}", refusal.next), width, 2);
                if !refusal.output.is_empty() {
                    out.push(format!("{name} printed:"));
                    let room = height.saturating_sub(out.len());
                    push_output(&mut out, &refusal.output, width, room);
                }
            }
            Stage::Done(outcome) => {
                push_wrapped(&mut out, &outcome.line, width, 6);
                out.push(match &outcome.recorded {
                    Ok(()) => "Recorded in swamp's ledger.".to_string(),
                    Err(e) => format!("Could not record in swamp's ledger: {e}"),
                });
                out.push(
                    "swamp's stored sizes still show the last observation; R measures again."
                        .into(),
                );
            }
        }
        fit(out, width, height)
    }

    fn choose_lines(&self, width: usize, height: usize) -> Vec<String> {
        let mut out = Vec::new();
        let Some(listing) = &self.listing else {
            return fit(out, width, height);
        };
        out.push(format!(
            "{}'s own list. Pick one to review; nothing runs until you confirm.",
            self.manager.name()
        ));
        if listing.candidates.is_empty() {
            out.push(format!("{} lists nothing here.", self.manager.name()));
            return fit(out, width, height);
        }
        let rows = height.saturating_sub(out.len()).max(1);
        let first = self.cursor.saturating_sub(rows.saturating_sub(1));
        for (i, c) in listing.candidates.iter().enumerate().skip(first).take(rows) {
            let marker = if i == self.cursor { "▸ " } else { "  " };
            let line = if c.facts.is_empty() {
                format!("{marker}{}", c.label)
            } else {
                format!("{marker}{}  {}", c.label, c.facts)
            };
            out.push(line);
        }
        fit(out, width, height)
    }
}

/// The confirm. Its first four rows never move: what, no Trash, the
/// command heading, the command.
pub fn confirm_lines(p: &Preview, width: usize, height: usize) -> Vec<String> {
    let name = p.manager().name();
    let mut out: Vec<String> = vec![
        format!("Remove {} with {name}, permanently.", p.title()),
        "No Trash recovery: this cannot be undone.".to_string(),
        "Command (Y runs exactly this):".to_string(),
    ];
    // The command is never cut: it wraps under itself.
    push_wrapped(
        &mut out,
        &format!("  {}", p.command_line()),
        width,
        usize::MAX,
    );
    out.push(truncate_middle(
        &format!("Program: {}", p.program_path().display()),
        width,
    ));
    out.push(match p.size() {
        Some(s) => format!("Size: {} ({})", human_bytes(s.bytes), s.source),
        None => "Size: not measured (swamp has no stored measurement of it)".to_string(),
    });
    push_wrapped(&mut out, p.regen(), width, 2);
    push_wrapped(
        &mut out,
        &format!("Open files: {}", p.open_files()),
        width,
        2,
    );
    for e in p.evidence() {
        push_wrapped(&mut out, e, width, 2);
    }
    for w in p.warnings() {
        push_wrapped(&mut out, &format!("Warning: {w}"), width, 3);
    }
    // What the dry run names, parsed, before the raw text: a line of
    // manager output cannot hide what is removed.
    let removes = p.removes();
    if let Some(first) = removes.first() {
        let more = removes.len() - 1;
        out.push(cut(
            &if more > 0 {
                format!("Removes: {first} (+{more} more in the dry run below)")
            } else {
                format!("Removes: {first}")
            },
            width,
        ));
    }
    out.push(format!("{name}'s dry run (verbatim):"));
    let room = height.saturating_sub(out.len());
    push_output(&mut out, p.dry_output(), width, room);
    fit(out, width, height)
}

/// Rows of the sheet's body at terminal size `w`x`h`: the popup leaves a
/// one-cell margin and a border on each side, and the keys take the last
/// inner row.
pub fn body_size(w: u16, h: u16) -> (usize, usize) {
    (
        usize::from(w.saturating_sub(4)),
        usize::from(h.saturating_sub(4)).saturating_sub(1),
    )
}

/// Whether everything above the dry run (what, no Trash, the whole
/// command, size, cost, open files, evidence, warnings) and the dry run's
/// heading fit on a `w`x`h` terminal. Enter runs nothing until they do.
pub fn confirm_fits(p: &Preview, w: u16, h: u16) -> bool {
    let (width, rows) = body_size(w, h);
    let all = confirm_lines(p, width, usize::MAX);
    let head = all.len() - p.dry_output().len().min(all.len());
    head <= rows
}

/// `lines` indented two cells, cut to `width`, in at most `room` rows;
/// the last row says how many did not fit.
fn push_output(out: &mut Vec<String>, lines: &[String], width: usize, room: usize) {
    if room == 0 {
        return;
    }
    let shown = if lines.len() > room {
        room.saturating_sub(1)
    } else {
        lines.len()
    };
    for l in &lines[..shown] {
        out.push(cut(&format!("  {l}"), width));
    }
    if shown < lines.len() {
        out.push(format!("  +{} more lines not shown", lines.len() - shown));
    }
}

/// `text` wrapped at `width` in at most `max` rows (the last cut), with
/// continuation rows indented four cells.
fn push_wrapped(out: &mut Vec<String>, text: &str, width: usize, max: usize) {
    let width = width.max(8);
    let indent = text.len() - text.trim_start_matches(' ').len();
    let text = &text[indent..];
    let mut rows: Vec<String> = Vec::new();
    let lead = " ".repeat(indent);
    let mut cur = lead.clone();
    for word in text.split(' ') {
        let fresh = cur.trim().is_empty();
        let candidate = if fresh {
            format!("{cur}{word}")
        } else {
            format!("{cur} {word}")
        };
        if fresh || display_width(&candidate) <= width {
            cur = candidate;
            continue;
        }
        rows.push(std::mem::take(&mut cur));
        cur = format!("{lead}    {word}");
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    // A single word longer than the width is split by cells.
    let rows: Vec<String> = rows
        .into_iter()
        .flat_map(|r| split_cells(&r, width))
        .collect();
    if rows.len() > max {
        out.extend(rows[..max - 1].iter().cloned());
        out.push(cut(&rows[max - 1..].join(" "), width));
    } else {
        out.extend(rows);
    }
}

fn split_cells(s: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width {
            rows.push(std::mem::take(&mut cur));
            cur.push_str("    ");
            w = 4;
        }
        cur.push(c);
        w += cw;
    }
    rows.push(cur);
    rows
}

/// `s` cut to `width` cells, ending in `…` when cut.
fn cut(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if display_width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// At most `height` rows, each cut to `width`; when rows had to go, the
/// last row says so.
fn fit(mut lines: Vec<String>, width: usize, height: usize) -> Vec<String> {
    for l in lines.iter_mut() {
        *l = cut(l, width);
    }
    if lines.len() > height && height > 0 {
        lines.truncate(height - 1);
        lines.push(cut("(enlarge the terminal to see the rest)", width));
    }
    lines
}
