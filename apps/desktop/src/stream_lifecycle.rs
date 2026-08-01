// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Mutex;

pub(crate) type ActiveStream<C> = Mutex<Option<(u64, C)>>;

pub(crate) fn cancel<C>(active: &ActiveStream<C>, shutdown: impl FnOnce(&C)) {
    if let Ok(mut active) = active.lock()
        && let Some((_, control)) = active.take()
    {
        shutdown(&control);
    }
}

pub(crate) fn replace<C>(
    active: &ActiveStream<C>,
    generation: u64,
    control: C,
    shutdown: impl FnOnce(&C),
) {
    if let Ok(mut active) = active.lock()
        && let Some((_, old_control)) = active.replace((generation, control))
    {
        shutdown(&old_control);
    }
}

pub(crate) fn unregister<C>(active: &ActiveStream<C>, generation: u64) {
    if let Ok(mut active) = active.lock()
        && active
            .as_ref()
            .is_some_and(|(active_generation, _)| *active_generation == generation)
    {
        active.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug)]
    struct FakeControl {
        id: u64,
        shutdowns: Arc<Mutex<Vec<u64>>>,
    }

    impl FakeControl {
        fn new(id: u64, shutdowns: &Arc<Mutex<Vec<u64>>>) -> Self {
            Self {
                id,
                shutdowns: Arc::clone(shutdowns),
            }
        }

        fn shutdown(&self) {
            self.shutdowns.lock().unwrap().push(self.id);
        }
    }

    #[test]
    fn start_replace_stop_and_reconnect_shutdown_only_owned_controls() {
        let active = ActiveStream::default();
        let shutdowns = Arc::new(Mutex::new(Vec::new()));

        replace(
            &active,
            1,
            FakeControl::new(1, &shutdowns),
            FakeControl::shutdown,
        );
        assert_eq!(active.lock().unwrap().as_ref().map(|entry| entry.0), Some(1));
        assert!(shutdowns.lock().unwrap().is_empty());

        replace(
            &active,
            2,
            FakeControl::new(2, &shutdowns),
            FakeControl::shutdown,
        );
        assert_eq!(*shutdowns.lock().unwrap(), [1]);
        assert_eq!(active.lock().unwrap().as_ref().map(|entry| entry.0), Some(2));

        cancel(&active, FakeControl::shutdown);
        assert_eq!(*shutdowns.lock().unwrap(), [1, 2]);
        assert!(active.lock().unwrap().is_none());

        replace(
            &active,
            3,
            FakeControl::new(3, &shutdowns),
            FakeControl::shutdown,
        );
        cancel(&active, FakeControl::shutdown);
        assert_eq!(*shutdowns.lock().unwrap(), [1, 2, 3]);
    }

    #[test]
    fn stale_generation_cannot_unregister_replacement_stream() {
        let active = ActiveStream::default();
        let shutdowns = Arc::new(Mutex::new(Vec::new()));
        replace(
            &active,
            8,
            FakeControl::new(8, &shutdowns),
            FakeControl::shutdown,
        );

        unregister(&active, 7);
        assert_eq!(active.lock().unwrap().as_ref().map(|entry| entry.0), Some(8));
        unregister(&active, 8);
        assert!(active.lock().unwrap().is_none());
        assert!(shutdowns.lock().unwrap().is_empty());
    }

    #[test]
    fn unregistering_completed_stream_does_not_shutdown_it_twice() {
        let active = ActiveStream::default();
        let shutdowns = Arc::new(Mutex::new(Vec::new()));
        replace(
            &active,
            4,
            FakeControl::new(4, &shutdowns),
            FakeControl::shutdown,
        );
        unregister(&active, 4);
        cancel(&active, FakeControl::shutdown);
        assert!(shutdowns.lock().unwrap().is_empty());
    }

    #[test]
    fn poisoned_lifecycle_state_never_panics_during_cleanup() {
        let active = ActiveStream::<FakeControl>::default();
        let _ = std::panic::catch_unwind(|| {
            let _guard = active.lock().unwrap();
            panic!("poison lifecycle fixture");
        });
        cancel(&active, FakeControl::shutdown);
        unregister(&active, 1);
    }
}
