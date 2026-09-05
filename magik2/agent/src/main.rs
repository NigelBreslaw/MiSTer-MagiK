// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik2_agent::Agent;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("MISTER_MAGIK2_PORT").unwrap_or_else(|_| "7500".to_owned());
    let token =
        std::env::var("MISTER_MAGIK2_TOKEN").map_err(|_| "MISTER_MAGIK2_TOKEN is required")?;
    let root = std::env::var("MISTER_MAGIK2_INSTALL_ROOT")
        .unwrap_or_else(|_| "/media/fat/mister-magik2".to_owned());
    let state_root = std::env::var("MISTER_MAGIK2_STATE_ROOT")
        .unwrap_or_else(|_| "/tmp/mister-magik2".to_owned());
    let agent = Arc::new(Agent::with_state_root(
        env!("CARGO_PKG_VERSION").to_owned(),
        token,
        PathBuf::from(root),
        PathBuf::from(state_root),
    ));
    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))?;
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                let agent = agent.clone();
                std::thread::spawn(move || {
                    if let Err(error) = agent.handle(&mut stream) {
                        eprintln!("magik2 request rejected: {error:?}");
                    }
                });
            }
            Err(error) => eprintln!("magik2 accept failed: {error}"),
        }
    }
    Ok(())
}
