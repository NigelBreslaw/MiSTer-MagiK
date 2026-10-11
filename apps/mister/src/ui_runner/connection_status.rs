// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ConnectionState {
    pub internet: bool,
    pub usb: bool,
}

enum ConnectionEvent {
    Internet(bool),
    Usb(bool),
}

/// Bounded, independent probes: neither network nor mount reads run on the UI thread.
pub(super) struct ConnectionStatus {
    rx: Receiver<ConnectionEvent>,
    state: ConnectionState,
}

impl ConnectionStatus {
    pub(super) fn start() -> Self {
        let (tx, rx) = mpsc::sync_channel(2);
        let internet_tx = tx.clone();
        let internet = std::thread::Builder::new()
            .name("magik-internet-status".into())
            .spawn(move || {
                loop {
                    let online = internet_connected();
                    if internet_tx.send(ConnectionEvent::Internet(online)).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(30));
                }
            });
        let usb = std::thread::Builder::new()
            .name("magik-usb-status".into())
            .spawn(move || {
                loop {
                    let mounted = mister_magik_mister_runtime::usb_storage::connected();
                    if tx.send(ConnectionEvent::Usb(mounted)).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            });
        if let Err(error) = internet {
            crate::ui_errln!("internet status worker: {error}");
        }
        if let Err(error) = usb {
            crate::ui_errln!("USB status worker: {error}");
        }
        Self {
            rx,
            state: ConnectionState::default(),
        }
    }

    pub(super) fn poll(&mut self) -> Option<ConnectionState> {
        let previous = self.state;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                ConnectionEvent::Internet(online) => self.state.internet = online,
                ConnectionEvent::Usb(mounted) => self.state.usb = mounted,
            }
        }
        (previous != self.state).then_some(self.state)
    }
}

fn internet_connected() -> bool {
    Command::new("curl")
        .args([
            "--head",
            "--fail",
            "--silent",
            "--location",
            "--connect-timeout",
            "2",
            "--max-time",
            "4",
            "--output",
            "/dev/null",
            "https://assets.mistermagik.com/mister-magik/v1/manifest.json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_probe_events_update_and_clear_status_without_spurious_redraws() {
        let (tx, rx) = mpsc::sync_channel(2);
        let mut status = ConnectionStatus {
            rx,
            state: ConnectionState::default(),
        };
        tx.send(ConnectionEvent::Usb(true)).unwrap();
        assert_eq!(
            status.poll(),
            Some(ConnectionState {
                internet: false,
                usb: true
            })
        );
        tx.send(ConnectionEvent::Internet(true)).unwrap();
        assert_eq!(
            status.poll(),
            Some(ConnectionState {
                internet: true,
                usb: true
            })
        );
        tx.send(ConnectionEvent::Internet(true)).unwrap();
        assert_eq!(status.poll(), None);
        tx.send(ConnectionEvent::Usb(false)).unwrap();
        tx.send(ConnectionEvent::Internet(false)).unwrap();
        assert_eq!(status.poll(), Some(ConnectionState::default()));
        assert_eq!(status.poll(), None);
    }
}
