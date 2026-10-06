//! A real terminal must keep accepting keys when descriptor zero disappears.
#![cfg(unix)]
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn terminal_input_child() {
    if std::env::var_os("SWAMP_TERMINAL_INPUT_CHILD").is_none() {
        return;
    }
    let guard = swamp_tui::term::TerminalGuard::enter().unwrap();
    // Reproduce the live hung UI: its stdin had been closed/reused, while
    // stdout and stderr still pointed at the controlling terminal.
    assert_eq!(unsafe { libc::close(0) }, 0);
    println!("INPUT_READY");
    std::io::stdout().flush().unwrap();
    for expected in [
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyCode::Char('q'),
    ] {
        assert!(crossterm::event::poll(Duration::from_secs(3)).unwrap());
        let crossterm::event::Event::Key(key) = crossterm::event::read().unwrap() else {
            panic!("expected a key");
        };
        assert_eq!(key.code, expected);
    }
    drop(guard);
    println!("INPUT_PASSED");
}

#[test]
fn arrows_and_quit_survive_closed_stdin() {
    use std::os::unix::process::CommandExt;
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let tty_fd = slave.as_raw_fd();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "terminal_input_child", "--nocapture"])
        .env("SWAMP_TERMINAL_INPUT_CHILD", "1")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 || libc::ioctl(tty_fd, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
        libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = String::new();
    let mut sent = false;
    loop {
        let mut bytes = [0; 4096];
        if let Ok(n) = master.read(&mut bytes) {
            output.push_str(&String::from_utf8_lossy(&bytes[..n]));
        }
        if !sent && output.contains("INPUT_READY") {
            master.write_all(b"\x1b[Bq").unwrap();
            sent = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{output}");
            assert!(output.contains("INPUT_PASSED"), "{output}");
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("terminal ignored input: {output}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
