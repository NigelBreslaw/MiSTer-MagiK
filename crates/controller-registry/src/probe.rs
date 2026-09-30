// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Clone)]
pub struct SaveProbe(std::sync::Arc<dyn Fn(SavePhase) + Send + Sync>);
impl std::fmt::Debug for SaveProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SaveProbe")
    }
}
#[derive(Clone, Copy, Debug)]
pub enum SavePhase {
    Started,
    Finished(bool),
}
impl SaveProbe {
    pub fn new(f: impl Fn(SavePhase) + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(f))
    }
    pub fn observe(&self, phase: SavePhase) {
        (self.0)(phase);
    }
}
