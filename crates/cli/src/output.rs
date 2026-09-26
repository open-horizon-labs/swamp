//! Fallible terminal output for the CLI.

use std::fmt;
use std::io::{self, Write};

pub fn write_stdout(args: std::fmt::Arguments<'_>) -> anyhow::Result<()> {
    let stdout = io::stdout();
    write_to(&mut stdout.lock(), args)?;
    Ok(())
}

pub fn write_stdout_line(args: fmt::Arguments<'_>) -> anyhow::Result<()> {
    write_stdout(format_args!("{}", Line(args)))
}

struct Line<'a>(fmt::Arguments<'a>);

impl fmt::Display for Line<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_fmt(self.0)?;
        formatter.write_str("\n")
    }
}

fn write_to(writer: &mut impl Write, args: std::fmt::Arguments<'_>) -> io::Result<()> {
    match writer.write_fmt(args) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::write_to;
    use std::io::{self, Write};

    struct FailingWriter(io::ErrorKind);

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "injected writer failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn closed_pipe_is_not_an_output_error() {
        assert!(
            write_to(
                &mut FailingWriter(io::ErrorKind::BrokenPipe),
                format_args!("output")
            )
            .is_ok()
        );
    }

    #[test]
    fn other_output_errors_are_returned() {
        let error = write_to(
            &mut FailingWriter(io::ErrorKind::PermissionDenied),
            format_args!("output"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
}
