// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Frame state shared by the launcher navigation and RGB565 renderer.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowseDirection {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowsePhase {
    Settled,
    Flipping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrowseFrame {
    pub selected: usize,
    pub target: usize,
    pub phase: BrowsePhase,
    pub direction: Option<BrowseDirection>,
    /// Elapsed timeline milliseconds, or integrated position units when
    /// `duration_millis == SPRING_POSITION_UNITS` (not eased a second time).
    pub progress_millis: u32,
    pub duration_millis: u32,
}

pub const SPRING_POSITION_UNITS: u32 = 65536;
