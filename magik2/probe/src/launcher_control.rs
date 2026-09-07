// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Consumer boundary: physical and test input use the same portable browser.
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, LauncherBrowser,
};

pub struct LauncherControl {
    browser: LauncherBrowser,
    count: usize,
    held: [[bool; 2]; 2],
    pub rearm_input: bool,
}

impl LauncherControl {
    pub fn new(count: usize) -> Self {
        let mut browser = LauncherBrowser::new(count, 0);
        browser.neutral();
        Self {
            browser,
            count,
            held: [[false; 2]; 2],
            rearm_input: true,
        }
    }

    pub fn reset(&mut self, home: bool) {
        let selected = if home { 0 } else { self.browser.selected() };
        self.browser = LauncherBrowser::new(self.count, selected);
        self.browser.neutral();
        self.held = [[false; 2]; 2];
        // Reopening the physical reader obtains a fresh authoritative held-key
        // snapshot; it suppresses inherited holds until neutral independently.
        self.rearm_input = true;
    }

    pub fn input(&mut self, physical: bool, direction: BrowseDirection, down: bool, now: u64) {
        let index = usize::from(direction == BrowseDirection::Right);
        let previous = self.held[0][index] || self.held[1][index];
        self.held[usize::from(physical)][index] = down;
        let next = self.held[0][index] || self.held[1][index];
        if next && !previous {
            self.browser.press(direction, now);
        }
        if previous && !next {
            self.browser.release(direction);
        }
    }

    pub fn frame(&mut self, now: u64) -> BrowseFrame {
        self.browser.frame(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_framebuffer_scenes::launcher_navigation::BrowsePhase;

    #[test]
    fn test_release_cannot_release_physical_hold() {
        let mut control = LauncherControl::new(5);
        control.input(true, BrowseDirection::Right, true, 0);
        control.input(false, BrowseDirection::Right, true, 20);
        control.input(false, BrowseDirection::Right, false, 30);
        control.frame(180);
        assert_eq!(control.frame(300).phase, BrowsePhase::Held);
        control.input(true, BrowseDirection::Right, false, 320);
        assert_eq!(control.frame(450).selected, 2);
        assert_eq!(control.frame(1000).phase, BrowsePhase::Settled);
    }

    #[test]
    fn mode_or_disconnect_reset_clears_all_pending_input() {
        let mut control = LauncherControl::new(5);
        control.input(true, BrowseDirection::Left, true, 0);
        control.reset(false);
        assert!(control.rearm_input);
        assert_eq!(control.frame(1000).phase, BrowsePhase::Settled);
        control.input(false, BrowseDirection::Right, true, 1001);
        control.input(false, BrowseDirection::Right, false, 1002);
        assert_eq!(control.frame(1181).selected, 1);
    }
}
