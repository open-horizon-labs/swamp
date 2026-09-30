//! Putting the terminal back when a full-screen program is killed.
//!
//! A program on the alternate screen in raw mode that dies of SIGTERM,
//! SIGHUP or SIGINT without restoring leaves the user's shell with no
//! echo and no cursor. The TUI arms this before it changes the terminal:
//! the current settings are saved once, and the fatal-signal handler and
//! the exit hook of [`super::spawn`] put them back with async-signal-safe
//! calls only (`write`, `tcsetattr`) before they kill the children and
//! re-deliver the signal.
//!
//! Not a blocking capability: arming and disarming are a `tcgetattr` and
//! a flag, safe to call from the event thread.

#![allow(unsafe_code)]

/// Leave the alternate screen, show the cursor, reset attributes.
const RESTORE: &[u8] = b"\x1b[?2004l\x1b[?1049l\x1b[?25h\x1b[0m";

struct Saved(std::cell::UnsafeCell<std::mem::MaybeUninit<libc::termios>>);
// SAFETY: written once, while `ARMED` is false, before it is set with
// Release; readers (the signal handler) load it with Acquire first.
unsafe impl Sync for Saved {}

static SAVED: Saved = Saved(std::cell::UnsafeCell::new(std::mem::MaybeUninit::uninit()));
static ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Saves the terminal's current settings so a fatal signal or process exit
/// can restore them, and makes sure the signal handlers are installed.
/// Call it before entering raw mode. `false` when stdin is not a terminal
/// (nothing to restore).
pub fn arm() -> bool {
    use std::sync::atomic::Ordering;
    super::spawn::install_cleanup_once();
    if ARMED.load(Ordering::Acquire) {
        return true;
    }
    let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr fills `t` when it returns 0.
    if unsafe { libc::tcgetattr(0, t.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: no reader touches the slot while `ARMED` is false.
    unsafe { (*SAVED.0.get()).write(t.assume_init()) };
    ARMED.store(true, Ordering::Release);
    true
}

/// The terminal has been restored by the normal path; the signal handler
/// and the exit hook leave it alone from here on.
pub fn disarm() {
    ARMED.store(false, std::sync::atomic::Ordering::Release);
}

/// Async-signal-safe: `write` and `tcsetattr` only.
pub(super) fn restore_signal_safe() {
    if !ARMED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    // SAFETY: the slot was fully written before the flag was set; both
    // calls are on the async-signal-safe list.
    unsafe {
        libc::write(1, RESTORE.as_ptr().cast::<libc::c_void>(), RESTORE.len());
        libc::tcsetattr(0, libc::TCSANOW, (*SAVED.0.get()).as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restore_sequence_leaves_the_alternate_screen_and_shows_the_cursor() {
        let s = std::str::from_utf8(RESTORE).unwrap();
        assert!(s.contains("\x1b[?1049l"), "leave the alternate screen");
        assert!(s.contains("\x1b[?25h"), "show the cursor");
        // Not armed: the handler path writes nothing and touches nothing.
        disarm();
        restore_signal_safe();
    }
}
