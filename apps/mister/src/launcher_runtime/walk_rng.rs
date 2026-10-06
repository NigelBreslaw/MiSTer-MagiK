// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! A tiny seeded generator for the random-walk scenarios, so a failing walk
//! reproduces from its number alone.

pub(super) struct WalkRng(pub(super) u64);

impl WalkRng {
    pub(super) fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }

    pub(super) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}
