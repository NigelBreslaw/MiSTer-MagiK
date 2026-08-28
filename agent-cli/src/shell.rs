// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub(crate) fn agent_retry_command(args: &[String]) -> String {
    let entrypoint = if args.get(1).map(String::as_str) == Some("ci") {
        "scripts/magik-ci"
    } else {
        "scripts/agent"
    };
    std::iter::once(entrypoint.to_owned())
        .chain(args.iter().skip(1).map(|arg| quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./".contains(&byte))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_command_preserves_shell_arguments() {
        let args = [
            "agent-cli/target/debug/agent-cli",
            "ci",
            "host-assurance",
            "--paths",
            "Quoted path",
            "",
            "it's",
            "safe/path-1.2",
        ]
        .map(str::to_owned);
        assert_eq!(
            agent_retry_command(&args),
            "scripts/magik-ci ci host-assurance --paths 'Quoted path' '' 'it'\\''s' safe/path-1.2"
        );
    }
}
