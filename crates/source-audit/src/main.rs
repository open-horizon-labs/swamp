//! Source audits: the exact path-reference rules that complement the
//! capability gates (`crates/core/src/fs_gate`). `--list` prints the
//! names (what a guardrail or ADR may reference); a plain run executes
//! them all and fails on any broken rule, naming the file and line.

use swamp_source_audit::audits;

use std::path::{Path, PathBuf};

fn main() {
    let default_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for name in audits::names() {
            println!("{name}");
        }
        return;
    }
    // `--root <dir>` audits another checkout of this workspace.
    let mut root_buf = default_root;
    let mut only: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--root" && i + 1 < args.len() {
            root_buf = PathBuf::from(&args[i + 1]);
            i += 2;
            continue;
        }
        only.push(args[i].as_str());
        i += 1;
    }
    let mut failed = 0;
    for (name, result) in audits::run_all(&root_buf) {
        if !only.is_empty() && !only.contains(&name) {
            continue;
        }
        match result {
            Ok(()) => println!("ok    {name}"),
            Err(e) => {
                failed += 1;
                println!("FAIL  {name}: {e}");
            }
        }
    }
    if failed > 0 {
        eprintln!("{failed} audit(s) failed");
        std::process::exit(1);
    }
}
