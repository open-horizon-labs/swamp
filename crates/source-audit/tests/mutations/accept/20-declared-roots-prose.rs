//! target: crates/cli/src/main.rs
//! mode: append
//! expect: accept
//! source: accept, #168
//! why: telling the user how to declare a root, and that swamp never guesses one, names no inference source
/// Accept: the instruction a non-interactive first run prints.
fn sweep_accept_declare_roots_line() -> &'static str {
    "No source directories declared; declare yours with: swamp config add-root <path>"
}
