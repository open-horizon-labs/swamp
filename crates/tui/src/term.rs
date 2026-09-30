//! The terminal, put back however the program ends.
//!
//! [`TerminalGuard`] enters raw mode and the alternate screen and restores
//! both on every way out: a normal return and an error return (its
//! `Drop`), a panic on the UI thread (a hook that restores before the
//! message prints, so the message lands on the normal screen and stays
//! readable), and SIGTERM, SIGHUP or SIGINT (the gate's signal handler,
//! armed here, which chains to the child-kill handlers). A panic on a
//! worker thread is left alone: the UI keeps running.

use anyhow::Result;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::Paragraph;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Whether a guard is live, and which thread draws.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// `worker::current_thread_token` of the drawing thread; 0 while none.
static UI_THREAD: AtomicU64 = AtomicU64::new(0);

/// Raw mode off, alternate screen left, cursor shown. Safe to call when
/// none of that is on; errors are ignored because the terminal may be
/// gone (a closed ssh session).
pub fn restore_to(out: &mut impl Write) {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        out,
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = out.flush();
}

/// Whether a panic on `thread` should put the terminal back: only while a
/// guard is live and only on the thread that owns the screen.
fn panic_should_restore(active: bool, ui: u64, thread: u64) -> bool {
    active && ui != 0 && ui == thread
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if panic_should_restore(
                ACTIVE.load(Ordering::SeqCst),
                UI_THREAD.load(Ordering::SeqCst),
                crate::worker::current_thread_token(),
            ) {
                restore_to(&mut std::io::stdout());
            }
            previous(info);
        }));
    });
}

/// Owns the screen for the life of the UI.
pub struct TerminalGuard {
    pub terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl TerminalGuard {
    pub fn enter() -> Result<Self> {
        // Saved before any change, so a signal can put back exactly this.
        swamp_core::fs_gate::terminal::arm();
        install_panic_hook();
        UI_THREAD.store(crate::worker::current_thread_token(), Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        let built = (|| -> Result<Terminal<CrosstermBackend<std::io::Stdout>>> {
            crossterm::terminal::enable_raw_mode()?;
            let mut stdout = std::io::stdout();
            // Bracketed paste: a paste arrives as one `Event::Paste`, never
            // as keys, so pasted text cannot press `Y` on a confirm.
            crossterm::execute!(
                stdout,
                EnterAlternateScreen,
                crossterm::event::EnableBracketedPaste
            )?;
            Ok(Terminal::new(CrosstermBackend::new(stdout))?)
        })();
        match built {
            Ok(terminal) => Ok(TerminalGuard { terminal }),
            Err(e) => {
                // Half entered: undo whatever took.
                ACTIVE.store(false, Ordering::SeqCst);
                restore_to(&mut std::io::stdout());
                swamp_core::fs_gate::terminal::disarm();
                Err(e)
            }
        }
    }

    /// One line on an otherwise empty screen: the store is being read
    /// and nothing else can be drawn yet.
    pub fn splash(&mut self, text: &str) {
        let _ = self
            .terminal
            .draw(|f| f.render_widget(Paragraph::new(text.to_string()), f.area()));
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::SeqCst);
        restore_to(self.terminal.backend_mut());
        swamp_core::fs_gate::terminal::disarm();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_writes_leave_alternate_screen_and_show_cursor() {
        let mut out: Vec<u8> = Vec::new();
        restore_to(&mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.contains("\x1b[?1049l"),
            "leaves the alternate screen: {s:?}"
        );
        assert!(s.contains("\x1b[?25h"), "shows the cursor: {s:?}");
    }

    #[test]
    fn a_panic_restores_only_on_the_ui_thread_while_a_guard_is_live() {
        let me = crate::worker::current_thread_token();
        let other = std::thread::spawn(crate::worker::current_thread_token)
            .join()
            .unwrap();
        assert_ne!(me, other);
        assert!(panic_should_restore(true, me, me));
        assert!(!panic_should_restore(true, me, other), "a worker panic");
        assert!(!panic_should_restore(false, me, me), "no guard live");
        assert!(!panic_should_restore(true, 0, me));
    }
}
