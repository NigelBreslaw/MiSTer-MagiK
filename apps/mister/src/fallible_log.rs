// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Best-effort process logging.
//!
//! Rust's standard `println!` and `eprintln!` macros panic when stdout/stderr
//! writes fail. The supervised launcher sends those streams to `/tmp`, so a full
//! tmpfs must drop log lines instead of taking down the UI.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

#[doc(hidden)]
pub fn stdout_line(args: fmt::Arguments<'_>) -> io::Result<()> {
    let _guard = log_line_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let stdout = io::stdout();
    write_line(stdout.lock(), args)
}

#[allow(dead_code)]
#[doc(hidden)]
pub fn stdout(args: fmt::Arguments<'_>) -> io::Result<()> {
    let _guard = log_line_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let result = output.write_fmt(args);
    drop(output);
    result
}

#[doc(hidden)]
pub fn stderr_line(args: fmt::Arguments<'_>) -> io::Result<()> {
    let _guard = log_line_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
macro_rules! ui_logln {
    () => {{
        let _ = $crate::fallible_log::stdout_line(format_args!(""));
    }};
    ($($arg:tt)*) => {{
        let _ = $crate::fallible_log::stdout_line(format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! ui_log {
    ($($arg:tt)*) => {{
        let _ = $crate::fallible_log::stdout(format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! ui_errln {
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

    #[derive(Default)]
    struct SingleWriteWriter {
        writes: usize,
        bytes: Vec<u8>,
    }

    impl Write for SingleWriteWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.writes > 1 {
                return Err(io::Error::other("line used more than one write"));
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
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

    #[test]
    fn write_line_buffers_content_and_newline_into_one_write() {
        let mut writer = SingleWriteWriter::default();

        write_line(&mut writer, format_args!("startup_timing\t{}", "ready"))
            .expect("write buffered line");

        assert_eq!(writer.writes, 1);
        assert_eq!(writer.bytes, b"startup_timing\tready\n");
    }
}
