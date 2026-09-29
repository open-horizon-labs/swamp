//! Enters the TUI's terminal guard, draws one line, then panics on the UI
//! thread. Run it under a pty to see that the terminal comes back (raw
//! mode off, alternate screen left, cursor shown) and the panic message is
//! readable on the normal screen. It is a probe for a person, not part of
//! `swamp`.

fn main() {
    let mut guard = swamp_tui::term::TerminalGuard::enter().expect("a terminal");
    guard.splash("swamp panic probe: about to panic on the UI thread");
    std::thread::sleep(std::time::Duration::from_millis(400));
    panic!("probe panic");
}
