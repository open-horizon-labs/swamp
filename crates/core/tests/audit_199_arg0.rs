//! Auditor (#199): a `Plan::Fixed` program is exec'd by its
//! symlink-resolved path, so a multi-call binary reached through a link
//! (OrbStack's `docker` -> `docker-tools`) only works if the child's
//! argv[0] is the program's own name. No other test observes argv[0]:
//! dropping `arg0(..)` in `Running::start` keeps every test green while
//! `docker` stops working on an OrbStack Mac. Own process: it sets
//! SWAMP_TEST_PROGRAM_DIR.
//!
//! Tempting wrong patch this fails: `Command::new(exe)` without `.arg0`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use swamp_core::fs_gate::spawn::{Program, run};

#[test]
fn a_fixed_program_sees_its_own_name_as_argv0() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.c");
    let out = dir.path().join("argv0.txt");
    std::fs::write(
        &src,
        format!(
            "#include <stdio.h>\nint main(int c,char**v){{FILE*f=fopen(\"{}\",\"w\");fputs(v[0],f);fclose(f);return 0;}}\n",
            out.display()
        ),
    )
    .unwrap();
    let bin = dir.path().join("multi-call-tools");
    let cc = ["/usr/bin/cc", "/usr/bin/gcc"]
        .into_iter()
        .find(|c| std::path::Path::new(c).exists())
        .expect("a C compiler at /usr/bin/cc (macOS CLT, build-essential)");
    #[allow(clippy::disallowed_methods, clippy::disallowed_types)]
    let ok = std::process::Command::new(cc)
        .arg("-o")
        .arg(&bin)
        .arg(&src)
        .status()
        .unwrap()
        .success();
    assert!(ok, "cc failed");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The fake directory holds the binary under its own name; argv[0]
    // must still be the program's name, not the file it was run from.
    let fakes = dir.path().join("fakes");
    std::fs::create_dir(&fakes).unwrap();
    std::fs::copy(&bin, fakes.join("docker")).unwrap();
    // SAFETY: the only test in this binary.
    unsafe { std::env::set_var("SWAMP_TEST_PROGRAM_DIR", &fakes) };
    let r = run(
        Program::Docker,
        ["version", "--format", "json"],
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(r.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "docker");
}
