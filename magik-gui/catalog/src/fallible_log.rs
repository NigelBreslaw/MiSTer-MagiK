// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Best-effort logging for catalog work used by the supervised launcher.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

pub(crate) fn stdout_line(args: fmt::Arguments<'_>) -> io::Result<()> {
    let _guard = log_line_lock().lock().unwrap_or_else(|error| error.into_inner());
    stdout_line_unlocked(args)
}

fn stdout_line_unlocked(args: fmt::Arguments<'_>) -> io::Result<()> {
    if std::env::var_os("MISTER_CATALOG_PROTOCOL_STDOUT").is_some() {
        return stderr_line_unlocked(args);
    }
    let stdout = io::stdout();
    write_line(stdout.lock(), args)
}

pub(crate) fn stderr_line(args: fmt::Arguments<'_>) -> io::Result<()> {
    let _guard = log_line_lock().lock().unwrap_or_else(|error| error.into_inner());
    stderr_line_unlocked(args)
}

fn stderr_line_unlocked(args: fmt::Arguments<'_>) -> io::Result<()> {
    let stderr = io::stderr();
    write_line(stderr.lock(), args)
}

fn log_line_lock() -> &'static Mutex<()> {
    static LOG_LINE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOG_LINE_LOCK.get_or_init(|| Mutex::new(()))
}

fn write_line(mut writer: impl Write, args: fmt::Arguments<'_>) -> io::Result<()> {
    let mut line = Vec::new();
    line.write_fmt(args)?;
    line.push(b'\n');
    writer.write_all(&line)
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
