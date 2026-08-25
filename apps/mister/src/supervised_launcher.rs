// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::time::{Duration, Instant};

const MAX_MESSAGE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisedLauncherFds {
    status: RawFd,
    continuation: RawFd,
}

impl SupervisedLauncherFds {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        if args.len() != 4 {
            return Err("usage: supervised-launcher <status-fd> <continue-fd>".to_owned());
        }
        let status = parse_fd(&args[2], "status")?;
        let continuation = parse_fd(&args[3], "continue")?;
        if status == continuation {
            return Err("supervised launcher file descriptors must be distinct".to_owned());
        }
        Ok(Self {
            status,
            continuation,
        })
    }

    pub fn exchange(
        self,
        expected_token: &str,
        timeout: Duration,
    ) -> io::Result<SupervisedContinuation> {
        validate_token(expected_token)
            .then_some(())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid startup token"))?;

        // SAFETY: Main passes two private, inherited pipe descriptors and transfers their
        // ownership to this command. Parsing rejects stdio descriptors and duplicates.
        let mut status = unsafe { File::from_raw_fd(self.status) };
        // SAFETY: See the ownership contract above.
        let continuation = unsafe { File::from_raw_fd(self.continuation) };
        writeln!(
            status,
            "preflight-v1 ready pid={} token={expected_token}",
            std::process::id()
        )?;
        status.flush()?;
        drop(status);

        let line = read_line_before(&continuation, timeout)?;
        SupervisedContinuation::parse(&line, expected_token)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisedContinuation {
    pub main_pid: u32,
    pub main_generation: u64,
    pub owner_epoch: u64,
}

impl SupervisedContinuation {
    fn parse(line: &str, expected_token: &str) -> io::Result<Self> {
        let mut fields = line.trim_end_matches(['\n', '\r']).split_ascii_whitespace();
        if fields.next() != Some("continue-v1") {
            return Err(invalid_data("unsupported continuation protocol"));
        }
        let main_pid = parse_field(&mut fields, "main_pid")?;
        let main_generation = parse_field(&mut fields, "main_generation")?;
        let owner_epoch = parse_field(&mut fields, "owner_epoch")?;
        let token = parse_text_field(&mut fields, "token")?;
        if fields.next().is_some() || token != expected_token || !validate_token(token) {
            return Err(invalid_data("continuation authentication failed"));
        }
        if main_pid == 0 || main_generation == 0 || owner_epoch == 0 {
            return Err(invalid_data("continuation owner context is incomplete"));
        }
        Ok(Self {
            main_pid,
            main_generation,
            owner_epoch,
        })
    }
}

fn parse_fd(value: &str, label: &str) -> Result<RawFd, String> {
    value
        .parse::<RawFd>()
        .ok()
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| format!("invalid {label} file descriptor"))
}

fn parse_field<'a, T: std::str::FromStr>(
    fields: &mut impl Iterator<Item = &'a str>,
    name: &str,
) -> io::Result<T> {
    parse_text_field(fields, name)?
        .parse()
        .map_err(|_| invalid_data("invalid continuation number"))
}

fn parse_text_field<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    name: &str,
) -> io::Result<&'a str> {
    fields
        .next()
        .and_then(|field| field.strip_prefix(name))
        .and_then(|field| field.strip_prefix('='))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_data("malformed continuation field"))
}

fn validate_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_line_before(file: &File, timeout: Duration) -> io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::with_capacity(128);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "launcher continuation timed out",
            ));
        }
        let remaining_ms = deadline
            .saturating_duration_since(now)
            .as_millis()
            .clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor points to one valid pollfd for the duration of the call.
        let result = unsafe { libc::poll(&mut descriptor, 1, remaining_ms) };
        if result == 0 {
            continue;
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(invalid_data("launcher continuation pipe failed"));
        }

        let mut buffer = [0u8; 128];
        // SAFETY: buffer is writable and the polled descriptor remains owned by `file`.
        let count =
            unsafe { libc::read(file.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "launcher continuation pipe closed",
            ));
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        bytes.extend_from_slice(&buffer[..count as usize]);
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(invalid_data("launcher continuation is too long"));
        }
        if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
            if end + 1 != bytes.len() {
                return Err(invalid_data("launcher continuation has trailing data"));
            }
            return String::from_utf8(bytes).map_err(|_| invalid_data("continuation is not UTF-8"));
        }
    }
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn parses_exact_authenticated_continuation() {
        assert_eq!(
            SupervisedContinuation::parse(
                "continue-v1 main_pid=7 main_generation=11 owner_epoch=13 token=0123456789abcdef0123456789abcdef\n",
                TOKEN,
            )
            .unwrap(),
            SupervisedContinuation {
                main_pid: 7,
                main_generation: 11,
                owner_epoch: 13,
            }
        );
    }

    #[test]
    fn rejects_mismatched_token_and_trailing_fields() {
        assert!(SupervisedContinuation::parse(
            "continue-v1 main_pid=7 main_generation=11 owner_epoch=13 token=ffffffffffffffffffffffffffffffff\n",
            TOKEN,
        )
        .is_err());
        assert!(SupervisedContinuation::parse(
            "continue-v1 main_pid=7 main_generation=11 owner_epoch=13 token=0123456789abcdef0123456789abcdef extra=1\n",
            TOKEN,
        )
        .is_err());
    }
}
