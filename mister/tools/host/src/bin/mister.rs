// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    if let Err(error) = mister_tool::run_cli() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
