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
