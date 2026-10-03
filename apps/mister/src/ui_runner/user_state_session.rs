// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::legacy_user_state_import::import_legacy_snes;
pub(super) use mister_magik_catalog::user_state::UserStateSnapshot;
use mister_magik_catalog::user_state::{UserGameIdentity, UserStateStore};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

enum UserStateRequest {
    Refresh {
        legacy_catalog: Option<crate::arcade_catalog::ArcadeCatalog>,
        arcade_systems: Vec<String>,
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
    legacy_catalog_sent: bool,
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
            legacy_catalog_sent: false,
        }
    }

    pub(super) fn available(&self) -> bool {
        self.available
    }

    pub(super) fn refresh(
        &mut self,
        catalog: &crate::arcade_catalog::ArcadeCatalog,
        now: i64,
    ) -> Result<(), String> {
        let mut arcade_systems =
            catalog.search_source_system_ids(crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID);
        arcade_systems.sort_unstable();
        let legacy_catalog = (!self.legacy_catalog_sent).then(|| catalog.clone());
        self.submit(UserStateRequest::Refresh {
            legacy_catalog,
            arcade_systems,
            now,
        })?;
        self.legacy_catalog_sent = true;
        Ok(())
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
    let mut cached: Option<UserStateSnapshot> = None;
    let mut arcade_systems = Vec::new();
    let mut pending_import = None;
    let mut import_completed = false;
    while let Ok(request) = requests.recv() {
        let completed_favourite = match &request {
            UserStateRequest::SetFavourite { game, .. } => Some(game.launch_ref.clone()),
            UserStateRequest::Refresh { .. } => None,
        };
        let result = match request {
            UserStateRequest::Refresh {
                legacy_catalog,
                arcade_systems: members,
                now,
            } => (|| {
                if let Some(catalog) = legacy_catalog {
                    pending_import = Some(catalog);
                }
                if !import_completed {
                    let catalog = pending_import
                        .as_ref()
                        .ok_or("legacy import context missing")?;
                    let games = catalog
                        .games
                        .iter()
                        .filter(|game| game.system_id.eq_ignore_ascii_case("snes"))
                        .map(|game| catalog.user_game_identity_for_entry(game))
                        .collect::<Vec<_>>();
                    import_legacy_snes(&store, &games, &media_root, now)?;
                    import_completed = true;
                    pending_import = None;
                }
                arcade_systems = members;
                let snapshot = store.read_snapshot(&arcade_systems)?;
                cached = Some(snapshot.clone());
                Ok(snapshot)
            })(),
            UserStateRequest::SetFavourite {
                game,
                favourite,
                now,
            } => store.set_favourite(&game, favourite, now).and_then(|_| {
                let snapshot = match cached.as_mut() {
                    Some(snapshot) => {
                        store.refresh_favourites(snapshot, &game.system_id)?;
                        snapshot
                    }
                    None => cached.insert(store.read_snapshot(&arcade_systems)?),
                };
                Ok(snapshot.clone())
            }),
        };
        if result.is_err() && completed_favourite.is_some() {
            // A durable write may have succeeded before its projection failed.
            cached = None;
        }
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
            static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mister-magik-user-state-session-{}-{nonce}-{sequence}",
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
        session
            .refresh(
                &crate::arcade_catalog::ArcadeCatalog::new(PathBuf::new(), vec![], vec![]),
                10,
            )
            .unwrap();
        let event = poll_until(&mut session);
        assert!(
            matches!(event, UserStateEvent::Snapshot { .. }),
            "{event:?}"
        );
        session
    }

    #[test]
    fn only_the_first_refresh_queues_a_catalog_for_legacy_import() {
        let (request_tx, request_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut session = UserStateSession {
            requests: request_tx,
            events: event_rx,
            pending_favourites: HashSet::new(),
            available: true,
            legacy_catalog_sent: false,
        };
        let catalog = crate::arcade_catalog::ArcadeCatalog::new(PathBuf::new(), vec![], vec![]);
        session.refresh(&catalog, 10).unwrap();
        assert!(matches!(
            request_rx.recv().unwrap(),
            UserStateRequest::Refresh {
                legacy_catalog: Some(_),
                ..
            }
        ));
        session.refresh(&catalog, 20).unwrap();
        assert!(matches!(
            request_rx.recv().unwrap(),
            UserStateRequest::Refresh {
                legacy_catalog: None,
                ..
            }
        ));
    }

    #[test]
    fn refresh_observes_plays_and_favourites_written_by_another_connection() {
        let root = TestDirectory::new();
        let mut session = ready_session(&root);
        let store = UserStateStore::open(root.0.join("state.sqlite3")).unwrap();
        store.record_play(&game(), 20).unwrap();
        store.set_favourite(&game(), true, 20).unwrap();
        let catalog = crate::arcade_catalog::ArcadeCatalog::new(PathBuf::new(), vec![], vec![]);
        session.refresh(&catalog, 30).unwrap();
        let UserStateEvent::Snapshot { snapshot, .. } = poll_until(&mut session) else {
            panic!("snapshot");
        };
        assert_eq!(snapshot.recent_launch_refs, [game().launch_ref.clone()]);
        assert_eq!(snapshot.favourite_launch_refs, [game().launch_ref]);
    }

    #[test]
    fn an_early_favourite_does_not_skip_legacy_import_on_first_refresh() {
        let root = TestDirectory::new();
        std::fs::create_dir_all(root.0.join("config")).unwrap();
        std::fs::write(
            root.0.join("config/SNES_favorites.cfg"),
            "/media/fat/games/SNES/legacy.sfc\n",
        )
        .unwrap();
        let mut session = UserStateSession::start(root.0.join("state.sqlite3"), root.0.clone());
        session.set_favourite(game(), true, 10).unwrap();
        assert!(matches!(
            poll_until(&mut session),
            UserStateEvent::Snapshot { .. }
        ));
        let catalog = crate::test_support::arcade_catalog(
            vec![
                crate::test_support::arcade_game("Legacy")
                    .system_id("snes")
                    .path("/media/fat/games/SNES/legacy.sfc")
                    .build(),
            ],
            vec![crate::test_support::arcade_system("snes", 1)],
        );
        session.refresh(&catalog, 20).unwrap();
        let event = poll_until(&mut session);
        let UserStateEvent::Snapshot { snapshot, .. } = event else {
            panic!("{event:?}");
        };
        assert!(snapshot.favourite_launch_refs.contains(&game().launch_ref));
        assert!(
            snapshot
                .favourite_launch_refs
                .contains(&"/media/fat/games/SNES/legacy.sfc".into())
        );
    }

    #[test]
    fn catalog_membership_change_refreshes_cached_arcade_recents() {
        let root = TestDirectory::new();
        let store = UserStateStore::open(root.0.join("state.sqlite3")).unwrap();
        for (system, played_at) in [("cps1", 10), ("cps2", 20)] {
            store
                .record_play(
                    &UserGameIdentity {
                        system_id: system.into(),
                        stable_key: system.into(),
                        launch_ref: format!("{system}.mra"),
                        ..game()
                    },
                    played_at,
                )
                .unwrap();
        }
        let make_catalog = |systems: &[&str]| {
            crate::test_support::arcade_catalog(
                systems
                    .iter()
                    .map(|system| {
                        crate::test_support::arcade_game(*system)
                            .system_id(*system)
                            .path(format!("{system}.mra"))
                            .build()
                    })
                    .collect(),
                systems
                    .iter()
                    .map(|system| crate::test_support::arcade_system(*system, 1))
                    .collect(),
            )
        };
        let mut session = ready_session(&root);
        session.refresh(&make_catalog(&["cps1"]), 30).unwrap();
        let UserStateEvent::Snapshot { snapshot, .. } = poll_until(&mut session) else {
            panic!("snapshot");
        };
        assert_eq!(snapshot.arcade_recent_refs, ["cps1.mra"]);
        session
            .refresh(&make_catalog(&["cps1", "cps2"]), 40)
            .unwrap();
        let UserStateEvent::Snapshot { snapshot, .. } = poll_until(&mut session) else {
            panic!("snapshot");
        };
        assert_eq!(snapshot.arcade_recent_refs, ["cps2.mra", "cps1.mra"]);
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
            legacy_catalog_sent: false,
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
        assert!(
            session
                .refresh(
                    &crate::arcade_catalog::ArcadeCatalog::new(PathBuf::new(), vec![], vec![]),
                    40
                )
                .is_err()
        );
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
            legacy_catalog_sent: false,
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
            legacy_catalog_sent: false,
        };
        session.set_favourite(game(), true, 20).unwrap();
        drop(request_rx);
        assert!(
            session
                .refresh(
                    &crate::arcade_catalog::ArcadeCatalog::new(PathBuf::new(), vec![], vec![]),
                    30
                )
                .is_err()
        );
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
            legacy_catalog_sent: false,
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
