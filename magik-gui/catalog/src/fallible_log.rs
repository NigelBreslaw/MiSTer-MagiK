//! Best-effort logging for catalog work used by the supervised launcher.

use std::fmt;
use std::io::{self, Write};

pub(crate) fn stdout_line(args: fmt::Arguments<'_>) -> io::Result<()> {
    let stdout = io::stdout();
    write_line(stdout.lock(), args)
}

pub(crate) fn stderr_line(args: fmt::Arguments<'_>) -> io::Result<()> {
    let stderr = io::stderr();
    write_line(stderr.lock(), args)
}

fn write_line(mut writer: impl Write, args: fmt::Arguments<'_>) -> io::Result<()> {
    writer.write_fmt(args)?;
    writer.write_all(b"\n")
}

#[macro_export]
macro_rules! catalog_logln {
    () => {{
        let _ = $crate::fallible_log::stdout_line(format_args!(""));
    }};
    ($($arg:tt)*) => {{
        let _ = $crate::fallible_log::stdout_line(format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! catalog_errln {
    () => {{
        let _ = $crate::fallible_log::stderr_line(format_args!(""));
    }};
    ($($arg:tt)*) => {{
        let _ = $crate::fallible_log::stderr_line(format_args!($($arg)*));
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_line_reports_storage_failures_without_panicking() {
        let err = write_line(FailingWriter, format_args!("dropped {}", "line"))
            .expect_err("storage failure");

        assert_eq!(err.kind(), io::ErrorKind::StorageFull);
    }
}
