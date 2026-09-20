// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production owner for the custom RGB565 root launcher.

use crate::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use crate::launcher_home::{CARD_COUNT, LauncherHomeCard, LauncherHomeSnapshot};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::bitmap_text::BitmapFont;
use mister_magik_framebuffer_scenes::launcher::{
    LauncherData, LauncherScene, LauncherTypography, PreparedLauncher,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase, LauncherBrowser,
};

const CARD_WIDTH: usize = 180;
const CARD_HEIGHT: usize = 252;

const CARD_ASSETS: [&[u8]; CARD_COUNT] = [
    include_bytes!("../../assets/ui/launcher-cards/01_arcade.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/02_consoles.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/03_computers.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/04_handhelds.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/05_favourites.rgb565"),
    include_bytes!("../../assets/ui/launcher-cards/06_settings.rgb565"),
];

struct LauncherFonts {
    heading: BitmapFont,
    number: BitmapFont,
    metadata: BitmapFont,
    fallback: BitmapFont,
}

impl LauncherFonts {
    fn load() -> Result<Self, String> {
        Ok(Self {
            heading: launcher_bitmap_font(nocive_15_console_bitmap_font()?),
            number: launcher_bitmap_font(jersey_25_console_bitmap_font()?),
            metadata: launcher_bitmap_font(xerxes_10_console_bitmap_font()?),
            fallback: launcher_bitmap_font(spleen_6x12_native_console_bitmap_font()?),
        })
    }

    fn typography(&self) -> LauncherTypography<'_> {
        LauncherTypography {
            heading: &self.heading,
            number: &self.number,
            metadata: &self.metadata,
            fallback: &self.fallback,
        }
    }
}

pub(super) struct LauncherCardHomeSession {
    width: usize,
    height: usize,
    snapshot: LauncherHomeSnapshot,
    clock: String,
    artwork: [Vec<Rgb565Pixel>; CARD_COUNT],
    fonts: LauncherFonts,
    prepared: PreparedLauncher,
    browser: LauncherBrowser,
    desired_selection: usize,
    held_direction: Option<BrowseDirection>,
    frame: BrowseFrame,
    active: bool,
    content_dirty: bool,
}

impl LauncherCardHomeSession {
    pub(super) fn new(
        width: usize,
        height: usize,
        snapshot: LauncherHomeSnapshot,
        selected: usize,
        clock: &str,
    ) -> Result<Self, String> {
        let artwork = CARD_ASSETS.map(decode_card_asset);
        let fonts = LauncherFonts::load()?;
        let prepared = prepare(width, height, &snapshot, selected, clock, &artwork, &fonts);
        let mut browser = LauncherBrowser::new(CARD_COUNT, selected);
        browser.neutral();
        let frame = browser.frame(0);
        Ok(Self {
            width,
            height,
            snapshot,
            clock: clock.to_owned(),
            artwork,
            fonts,
            prepared,
            browser,
            desired_selection: selected,
            held_direction: None,
            frame,
            active: false,
            content_dirty: true,
        })
    }

    pub(super) fn set_inactive(&mut self) {
        self.active = false;
        self.held_direction = None;
    }

    pub(super) fn update(
        &mut self,
        width: usize,
        height: usize,
        snapshot: LauncherHomeSnapshot,
        selected: usize,
        held_direction: Option<BrowseDirection>,
        clock: &str,
        now_ms: u64,
    ) {
        let selected = selected.min(CARD_COUNT - 1);
        if !self.active {
            self.browser = LauncherBrowser::new(CARD_COUNT, selected);
            self.browser.neutral();
            self.desired_selection = selected;
            self.held_direction = None;
            self.active = true;
            self.content_dirty = true;
        }

        if self.held_direction != held_direction {
            if let Some(direction) = self.held_direction {
                self.browser.release_at(direction, now_ms);
            }
            if let Some(direction) = held_direction {
                self.browser.press(direction, now_ms);
            }
            self.held_direction = held_direction;
        }

        self.desired_selection = selected;
        self.frame = self.browser.frame(now_ms);
        if held_direction.is_none()
            && self.frame.phase == BrowsePhase::Settled
            && self.frame.selected != self.desired_selection
        {
            let direction = if self.frame.selected < self.desired_selection {
                BrowseDirection::Right
            } else {
                BrowseDirection::Left
            };
            self.browser.press(direction, now_ms);
            self.browser.release_at(direction, now_ms);
            self.frame = self.browser.frame(now_ms);
        }

        if self.width != width
            || self.height != height
            || self.snapshot != snapshot
            || self.clock != clock
        {
            self.width = width;
            self.height = height;
            self.snapshot = snapshot;
            self.clock.clear();
            self.clock.push_str(clock);
            self.prepared = prepare(
                width,
                height,
                &self.snapshot,
                self.frame.selected,
                &self.clock,
                &self.artwork,
                &self.fonts,
            );
            self.content_dirty = true;
        }
    }

    pub(super) fn is_animating(&self) -> bool {
        self.active && (self.frame.phase != BrowsePhase::Settled || self.frame.outgoing.is_some())
    }

    pub(super) const fn settled_selection(&self) -> Option<usize> {
        if matches!(self.frame.phase, BrowsePhase::Settled) {
            Some(self.frame.selected)
        } else {
            None
        }
    }

    pub(super) fn needs_render(&self) -> bool {
        self.active && (self.content_dirty || self.is_animating())
    }

    pub(super) fn render(&mut self) -> &[Rgb565Pixel] {
        self.prepared.render_frame(self.frame);
        self.content_dirty = false;
        self.prepared.pixels()
    }
}

fn prepare(
    width: usize,
    height: usize,
    snapshot: &LauncherHomeSnapshot,
    selected: usize,
    clock: &str,
    artwork: &[Vec<Rgb565Pixel>; CARD_COUNT],
    fonts: &LauncherFonts,
) -> PreparedLauncher {
    let cards = snapshot
        .cards
        .iter()
        .map(LauncherHomeCard::borrowed)
        .collect::<Vec<_>>();
    let artwork = artwork.iter().map(Vec::as_slice).collect::<Vec<_>>();
    LauncherScene::new(width, height)
        .prepare_initial_with_artwork_and_typography(
            LauncherData {
                cards: &cards,
                selected,
                library_games: snapshot.library_games,
                collections: snapshot.collections,
                favourites: snapshot.favourites,
                clock,
            },
            &artwork,
            fonts.typography(),
        )
        .finish()
}

fn decode_card_asset(bytes: &[u8]) -> Vec<Rgb565Pixel> {
    assert_eq!(bytes.len(), CARD_WIDTH * CARD_HEIGHT * 2);
    bytes
        .chunks_exact(2)
        .map(|pair| Rgb565Pixel(u16::from_le_bytes([pair[0], pair[1]])))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher_home::LauncherHomeCounts;

    fn snapshot() -> LauncherHomeSnapshot {
        LauncherHomeSnapshot::from_counts(LauncherHomeCounts {
            arcade: 1,
            consoles: 2,
            computers: 3,
            handhelds: 4,
            favourites: 5,
            library_games: 15,
            collections: 4,
        })
    }

    #[test]
    fn root_session_renders_exact_geometry_and_animates_toward_navigation() {
        let mut session = LauncherCardHomeSession::new(960, 540, snapshot(), 0, "21:37").unwrap();
        session.update(960, 540, snapshot(), 0, None, "21:37", 0);
        session.update(960, 540, snapshot(), 1, None, "21:37", 10);
        assert!(session.is_animating());
        assert_eq!(session.settled_selection(), None);
        assert_eq!(session.render().len(), 960 * 540);
    }
}
