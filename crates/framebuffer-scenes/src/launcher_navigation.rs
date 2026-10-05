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

pub const NESTED_STEP_MILLIS: u32 = 460;

pub const SPRING_POSITION_UNITS: u32 = 65536;

/// Navigation owns the identity of the card that starts a hierarchy change.
/// A BrowseFrame's `selected` is the outgoing half of a flip, not activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardLevelTransition {
    pub source_level: String,
    pub source_card: String,
    pub destination_level: String,
}

impl CardLevelTransition {
    /// Resolve identity against the retained source, never reuse a stale index.
    pub fn source_index<'a>(
        &self,
        source_level: &str,
        destination_level: &str,
        card_keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<usize> {
        if self.source_level != source_level || self.destination_level != destination_level {
            return None;
        }
        card_keys
            .into_iter()
            .position(|key| key == self.source_card)
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    #[test]
    fn origin_tracks_identity_through_reordering_and_rejects_wrong_levels_or_removed_cards() {
        let origin = CardLevelTransition {
            source_level: "root".into(),
            source_card: "computers".into(),
            destination_level: "computer-makers".into(),
        };
        assert_eq!(
            origin.source_index("root", "computer-makers", ["arcade", "computers"]),
            Some(1)
        );
        assert_eq!(
            origin.source_index("root", "computer-makers", ["computers", "arcade"]),
            Some(0)
        );
        assert_eq!(
            origin.source_index("root", "computer-makers", ["arcade"]),
            None
        );
        assert_eq!(
            origin.source_index("other", "computer-makers", ["computers"]),
            None
        );
        assert_eq!(origin.source_index("root", "other", ["computers"]), None);
    }
}
