// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::Cli;
use agent_cli::request::RawRequest;
use clap::Parser;

fn main() {
    let raw = RawRequest::capture(std::env::args_os());
    match Cli::try_parse() {
        Ok(cli) => {
            println!("request {}: {:?}", raw.id, cli.into_intent());
        }
        Err(error) => {
            eprintln!("request {} rejected: {error}", raw.id);
            std::process::exit(2);
        }
    }
}
