// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik2_agent::Agent;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    agent.start_observation_receiver()?;
    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))?;
    let active = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                if active.fetch_add(1, Ordering::Relaxed) >= 16 {
                    active.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                let active = active.clone();
                let agent = agent.clone();
                std::thread::spawn(move || {
                    if let Err(error) = agent.handle(&mut stream) {
                        eprintln!("magik2 request rejected: {error:?}");
                    }
                    active.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(error) => eprintln!("magik2 accept failed: {error}"),
        }
    }
    Ok(())
}
