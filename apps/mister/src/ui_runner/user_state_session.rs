// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::legacy_user_state_import::import_legacy_snes;
use mister_magik_catalog::user_state::{UserGameIdentity, UserStateStore};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct UserStateSnapshot {
    pub favourite_launch_refs: Vec<String>,
    pub recent_launch_refs: Vec<String>,
}

enum UserStateRequest {
    Refresh {
        games: Vec<UserGameIdentity>,
        now: i64,
    },
    SetFavourite {
        game: UserGameIdentity,
        favourite: bool,
        now: i64,
    },
}

pub(super) enum UserStateEvent {
    Snapshot(UserStateSnapshot),
    Failed {
        error: String,
        rollback: Option<(String, bool)>,
    },
}

pub(super) struct UserStateSession {
    requests: mpsc::Sender<UserStateRequest>,
    events: mpsc::Receiver<UserStateEvent>,
}

impl UserStateSession {
    pub(super) fn start(path: PathBuf, media_root: PathBuf) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::Builder::new()
            .name("user-state".to_string())
            .spawn(move || worker(path, media_root, request_rx, event_tx))
            .expect("spawn user-state worker");
        Self {
            requests: request_tx,
            events: event_rx,
        }
    }

    pub(super) fn refresh(&self, games: Vec<UserGameIdentity>, now: i64) {
        let _ = self.requests.send(UserStateRequest::Refresh { games, now });
    }

    pub(super) fn set_favourite(&self, game: UserGameIdentity, favourite: bool, now: i64) {
        let _ = self.requests.send(UserStateRequest::SetFavourite {
            game,
            favourite,
            now,
        });
    }

    pub(super) fn poll(&self) -> Option<UserStateEvent> {
        self.events.try_recv().ok()
    }
}

fn worker(
    path: PathBuf,
    media_root: PathBuf,
    requests: mpsc::Receiver<UserStateRequest>,
    events: mpsc::Sender<UserStateEvent>,
) {
    let store = match UserStateStore::open(path) {
        Ok(store) => store,
        Err(error) => {
            let _ = events.send(UserStateEvent::Failed {
                error,
                rollback: None,
            });
            return;
        }
    };
    while let Ok(request) = requests.recv() {
        let rollback = match &request {
            UserStateRequest::SetFavourite {
                game, favourite, ..
            } => Some((game.launch_ref.clone(), !favourite)),
            UserStateRequest::Refresh { .. } => None,
        };
        let result = match request {
            UserStateRequest::Refresh { games, now } => {
                import_legacy_snes(&store, &games, &media_root, now).and_then(|_| snapshot(&store))
            }
            UserStateRequest::SetFavourite {
                game,
                favourite,
                now,
            } => store
                .set_favourite(&game, favourite, now)
                .and_then(|_| snapshot(&store)),
        };
        let event = match result {
            Ok(snapshot) => UserStateEvent::Snapshot(snapshot),
            Err(error) => UserStateEvent::Failed { error, rollback },
        };
        if events.send(event).is_err() {
            break;
        }
    }
}

fn snapshot(store: &UserStateStore) -> Result<UserStateSnapshot, String> {
    Ok(UserStateSnapshot {
        favourite_launch_refs: store
            .favourite_games("snes")?
            .into_iter()
            .map(|game| game.launch_ref)
            .collect(),
        recent_launch_refs: store
            .recent_unique("snes", 16)?
            .into_iter()
            .map(|recent| recent.game.launch_ref)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn game() -> UserGameIdentity {
        UserGameIdentity {
            system_id: "snes".to_string(),
            stable_key: "one".to_string(),
            title: "One".to_string(),
            launch_ref: "/media/fat/games/SNES/one.sfc".to_string(),
            payload_path: "/media/fat/games/SNES/one.sfc".to_string(),
        }
    }

    #[test]
    fn worker_persists_favourite_and_returns_snapshot() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-user-state-session-{}-{nonce}",
            std::process::id()
        ));
        let session = UserStateSession::start(root.join("state.sqlite3"), root.clone());
        session.refresh(vec![game()], 10);
        let _ = poll_until(&session);
        session.set_favourite(game(), true, 20);
        let snapshot = poll_until(&session);
        assert_eq!(
            snapshot.favourite_launch_refs,
            vec!["/media/fat/games/SNES/one.sfc"]
        );
    }

    fn poll_until(session: &UserStateSession) -> UserStateSnapshot {
        for _ in 0..100 {
            match session.poll() {
                Some(UserStateEvent::Snapshot(snapshot)) => return snapshot,
                Some(UserStateEvent::Failed { error, .. }) => panic!("{error}"),
                None => thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("user-state worker did not reply")
    }
}
