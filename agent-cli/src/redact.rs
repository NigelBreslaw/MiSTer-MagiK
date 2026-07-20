// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsString;

const SECRET_FLAGS: &[&str] = &["--password", "--pass", "--token", "--secret", "--api-key"];

#[must_use]
pub fn redact_args(args: impl IntoIterator<Item = OsString>) -> Vec<String> {
    let mut redact_next = false;
    args.into_iter()
        .map(|arg| {
            let text = arg.to_string_lossy().into_owned();
            if redact_next {
                redact_next = false;
                return "[REDACTED]".into();
            }
            if SECRET_FLAGS.contains(&text.as_str()) {
                redact_next = true;
                return text;
            }
            if let Some((name, _)) = text.split_once('=') {
                if SECRET_FLAGS.contains(&name) {
                    return format!("{name}=[REDACTED]");
                }
            }
            text
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_separate_and_joined_secrets() {
        let args = ["agent-cli", "--token", "value", "--api-key=other", "lint"]
            .into_iter()
            .map(OsString::from);
        assert_eq!(
            redact_args(args),
            [
                "agent-cli",
                "--token",
                "[REDACTED]",
                "--api-key=[REDACTED]",
                "lint"
            ]
        );
    }
}
