// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::legacy_user_state_import::import_legacy_snes;
use mister_magik_catalog::user_state::{UserGameIdentity, UserStateStore};
use std::collections::HashSet;
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

#[derive(Debug)]
pub(super) enum UserStateEvent {
    Snapshot {
        snapshot: UserStateSnapshot,
        completed_favourite: Option<String>,
    },
    Failed {
        error: String,
        completed_favourite: Option<String>,
    },
    Unavailable {
        error: String,
    },
}

pub(super) struct UserStateSession {
    requests: mpsc::Sender<UserStateRequest>,
    events: mpsc::Receiver<UserStateEvent>,
    pending_favourites: HashSet<String>,
    available: bool,
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
            pending_favourites: HashSet::new(),
            available: true,
        }
    }

    pub(super) fn available(&self) -> bool {
        self.available
    }

    pub(super) fn refresh(&mut self, games: Vec<UserGameIdentity>, now: i64) -> Result<(), String> {
        self.submit(UserStateRequest::Refresh { games, now })
    }

    pub(super) fn set_favourite(
        &mut self,
        game: UserGameIdentity,
        favourite: bool,
        now: i64,
    ) -> Result<(), String> {
        if self.pending_favourites.contains(&game.launch_ref) {
            return Ok(());
        }
        let launch_ref = game.launch_ref.clone();
        self.submit(UserStateRequest::SetFavourite {
            game,
            favourite,
            now,
        })?;
        self.pending_favourites.insert(launch_ref);
        Ok(())
    }

    fn submit(&mut self, request: UserStateRequest) -> Result<(), String> {
        if !self.available {
            return Err("user-state worker unavailable".to_string());
        }
        self.requests.send(request).map_err(|_| {
            self.mark_unavailable();
            "user-state worker disconnected".to_string()
        })
    }

    fn mark_unavailable(&mut self) {
        self.available = false;
        self.pending_favourites.clear();
    }

    pub(super) fn poll(&mut self) -> Option<UserStateEvent> {
        if !self.available {
            return None;
        }
        let event = match self.events.try_recv() {
            Ok(event) => event,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => UserStateEvent::Unavailable {
                error: "user-state worker disconnected".to_string(),
            },
        };
        match &event {
            UserStateEvent::Snapshot {
                completed_favourite,
                ..
            }
            | UserStateEvent::Failed {
                completed_favourite,
                ..
            } => {
                if let Some(launch_ref) = completed_favourite {
                    self.pending_favourites.remove(launch_ref);
                }
            }
            UserStateEvent::Unavailable { .. } => self.mark_unavailable(),
        }
        Some(event)
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
            let _ = events.send(UserStateEvent::Unavailable { error });
            return;
        }
    };
    while let Ok(request) = requests.recv() {
        let completed_favourite = match &request {
            UserStateRequest::SetFavourite { game, .. } => Some(game.launch_ref.clone()),
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
            Ok(snapshot) => UserStateEvent::Snapshot {
                snapshot,
                completed_favourite,
            },
            Err(error) => UserStateEvent::Failed {
                error,
                completed_favourite,
            },
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
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "mister-magik-user-state-session-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn game() -> UserGameIdentity {
        UserGameIdentity {
            system_id: "snes".to_string(),
            stable_key: "one".to_string(),
            title: "One".to_string(),
            launch_ref: "/media/fat/games/SNES/one.sfc".to_string(),
            payload_path: "/media/fat/games/SNES/one.sfc".to_string(),
        }
    }

    fn poll_until(session: &mut UserStateSession) -> UserStateEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = session.poll() {
                return event;
            }
            assert!(Instant::now() < deadline, "user-state worker timed out");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn ready_session(root: &TestDirectory) -> UserStateSession {
        let mut session = UserStateSession::start(root.0.join("state.sqlite3"), root.0.clone());
        session.refresh(vec![game()], 10).unwrap();
        assert!(matches!(
            poll_until(&mut session),
            UserStateEvent::Snapshot { .. }
        ));
        session
    }

    #[test]
    fn worker_persists_before_returning_confirmed_snapshot() {
        let root = TestDirectory::new();
        let mut session = ready_session(&root);
        session.set_favourite(game(), true, 20).unwrap();
        assert!(session.pending_favourites.contains(&game().launch_ref));
        let UserStateEvent::Snapshot {
            snapshot,
            completed_favourite,
        } = poll_until(&mut session)
        else {
            panic!("favourite was not saved");
        };
        assert_eq!(completed_favourite, Some(game().launch_ref.clone()));
        assert_eq!(snapshot.favourite_launch_refs, vec![game().launch_ref]);
        assert!(session.pending_favourites.is_empty());
        let store = UserStateStore::open(root.0.join("state.sqlite3")).unwrap();
        assert!(store.is_favourite(&game()).unwrap());
        session.set_favourite(game(), false, 30).unwrap();
        let UserStateEvent::Snapshot { snapshot, .. } = poll_until(&mut session) else {
            panic!("favourite was not removed");
        };
        assert!(snapshot.favourite_launch_refs.is_empty());
        assert!(!store.is_favourite(&game()).unwrap());
    }

    #[test]
    fn initialization_failure_clears_queued_action_and_reports_unavailable_once() {
        let root = TestDirectory::new();
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let mut session = UserStateSession {
            requests: request_tx,
            events: event_rx,
            pending_favourites: HashSet::new(),
            available: true,
        };
        session.set_favourite(game(), true, 20).unwrap();
        // A directory cannot be opened as the SQLite database. Run the real
        // worker after submission to deterministically exercise queued work.
        worker(root.0.clone(), root.0.clone(), request_rx, event_tx);
        assert!(matches!(
            session.poll(),
            Some(UserStateEvent::Unavailable { .. })
        ));
        assert!(!session.available());
        assert!(session.pending_favourites.is_empty());
        assert!(session.poll().is_none());
        assert!(session.set_favourite(game(), true, 30).is_err());
        assert!(session.refresh(vec![game()], 40).is_err());
    }

    #[test]
    fn failed_write_returns_no_snapshot_and_releases_pending_action() {
        let root = TestDirectory::new();
        let mut session = ready_session(&root);
        let database = root.0.join("state.sqlite3");
        std::fs::remove_file(&database).unwrap();
        std::fs::create_dir(&database).unwrap();
        session.set_favourite(game(), true, 20).unwrap();
        let UserStateEvent::Failed {
            completed_favourite,
            ..
        } = poll_until(&mut session)
        else {
            panic!("failed write must not publish a snapshot");
        };
        assert_eq!(completed_favourite, Some(game().launch_ref));
        assert!(session.pending_favourites.is_empty());
        assert!(session.available());
        assert!(session.poll().is_none());
    }

    #[test]
    fn empty_queue_and_disconnected_worker_have_different_outcomes() {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let mut session = UserStateSession {
            requests: request_tx,
            events: event_rx,
            pending_favourites: HashSet::new(),
            available: true,
        };
        assert!(session.poll().is_none());
        assert!(session.available());
        session.set_favourite(game(), true, 20).unwrap();
        drop(event_tx);
        assert!(matches!(
            session.poll(),
            Some(UserStateEvent::Unavailable { .. })
        ));
        assert!(session.pending_favourites.is_empty());
        assert!(!session.available());
        assert!(session.poll().is_none());
        drop(request_rx);
    }

    #[test]
    fn submission_to_disconnected_worker_returns_error_and_clears_pending() {
        let (request_tx, request_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut session = UserStateSession {
            requests: request_tx,
            events: event_rx,
            pending_favourites: HashSet::new(),
            available: true,
        };
        session.set_favourite(game(), true, 20).unwrap();
        drop(request_rx);
        assert!(session.refresh(vec![game()], 30).is_err());
        assert!(!session.available());
        assert!(session.pending_favourites.is_empty());
        assert!(session.poll().is_none());
    }

    #[test]
    fn duplicate_pending_action_is_suppressed_until_its_own_reply() {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let mut session = UserStateSession {
            requests: request_tx,
            events: event_rx,
            pending_favourites: HashSet::new(),
            available: true,
        };
        session.set_favourite(game(), true, 20).unwrap();
        assert!(matches!(
            request_rx.try_recv(),
            Ok(UserStateRequest::SetFavourite { .. })
        ));
        session.set_favourite(game(), true, 21).unwrap();
        assert!(matches!(
            request_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        event_tx
            .send(UserStateEvent::Snapshot {
                snapshot: UserStateSnapshot::default(),
                completed_favourite: None,
            })
            .unwrap();
        assert!(matches!(
            session.poll(),
            Some(UserStateEvent::Snapshot { .. })
        ));
        session.set_favourite(game(), true, 22).unwrap();
        assert!(matches!(
            request_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        event_tx
            .send(UserStateEvent::Failed {
                error: "write failed".to_string(),
                completed_favourite: Some(game().launch_ref),
            })
            .unwrap();
        assert!(matches!(
            session.poll(),
            Some(UserStateEvent::Failed { .. })
        ));
        session.set_favourite(game(), true, 23).unwrap();
        assert!(matches!(
            request_rx.try_recv(),
            Ok(UserStateRequest::SetFavourite { .. })
        ));
    }
}
