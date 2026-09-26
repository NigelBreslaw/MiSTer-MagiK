// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Static text-and-colour launcher scene used by the Mini-MagiK visual probe.
//!
//! The scene deliberately has no Slint or runtime dependency. It renders a
//! packed RGB565 frame with native portrait/CRT layouts and the original
//! 960x540 landscape composition.

use crate::Rgb565Pixel;
use crate::bitmap_text::BitmapFont;
use std::sync::Arc;
mod artwork;
mod responsive;
use crate::launcher_navigation::{BrowseDirection, BrowseFrame};

pub const LOGICAL_WIDTH: usize = 960;
pub const LOGICAL_HEIGHT: usize = 540;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LauncherCardId {
    Arcade,
    Consoles,
    Computers,
    Handhelds,
    Favourites,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherCard<'a> {
    pub id: LauncherCardId,
    pub name: &'a str,
    pub games: Option<u32>,
    pub colour: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherData<'a> {
    pub cards: &'a [LauncherCard<'a>],
    pub selected: usize,
    pub library_games: u32,
    pub collections: u32,
    pub favourites: u32,
    pub clock: &'a str,
}

#[derive(Clone, Copy)]
pub struct LauncherTypography<'a> {
    pub heading: &'a BitmapFont,
    pub number: &'a BitmapFont,
    pub metadata: &'a BitmapFont,
    pub fallback: &'a BitmapFont,
}

impl LauncherTypography<'_> {
    fn font_for(&self, role: TextRole, text: &str) -> &BitmapFont {
        let primary = match role {
            TextRole::Heading => self.heading,
            TextRole::Number => self.number,
            TextRole::Metadata => self.metadata,
        };
        if text
            .chars()
            .all(|character| primary.glyph(character).is_some())
        {
            primary
        } else {
            self.fallback
        }
    }
}

#[derive(Clone, Copy)]
enum TextRole {
    Heading,
    Number,
    Metadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherScene {
    pub width: usize,
    pub height: usize,
    crt: bool,
    safe_insets: (usize, usize),
}

impl LauncherScene {
    #[must_use]
    pub const fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            crt: false,
            safe_insets: (0, 0),
        }
    }

    #[must_use]
    pub const fn uses_responsive_layout(self) -> bool {
        self.crt || self.height > self.width
    }

    /// CRT uses native bitmap text and a carousel-only composition in either orientation.
    #[must_use]
    pub const fn crt(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            crt: true,
            safe_insets: (0, 0),
        }
    }

    /// Keep route-owned PAL/overscan content insets, including after rotation.
    #[must_use]
    pub fn with_safe_content(mut self, content: crate::Rgb565Rect) -> Self {
        self.safe_insets = (
            content.x0.max(self.width.saturating_sub(content.x1)),
            content.y0.max(self.height.saturating_sub(content.y1)),
        );
        self
    }

    #[must_use]
    pub fn render(self, data: LauncherData<'_>) -> Vec<Rgb565Pixel> {
        self.render_browse(data, None)
    }

    #[must_use]
    pub fn render_browse(
        self,
        data: LauncherData<'_>,
        motion: Option<BrowseFrame>,
    ) -> Vec<Rgb565Pixel> {
        let frame = motion.unwrap_or(BrowseFrame {
            selected: data.selected,
            target: data.selected,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        });
        let mut output = vec![Rgb565Pixel(BACKGROUND); self.width.saturating_mul(self.height)];
        let mut prepared = self.prepare(data);
        prepared.render_into(frame, &mut output);
        output
    }

    #[must_use]
    pub fn prepare(self, data: LauncherData<'_>) -> PreparedLauncher {
        self.prepare_initial(data).finish()
    }

    /// Prepare a correct resting frame for the initial display handshake.
    /// `finish` retains the runtime's staged-entry contract; prepared textures
    /// and scratch buffers now cover all fractional sizes without a scale bank.
    pub fn prepare_initial(self, data: LauncherData<'_>) -> InitialLauncher {
        self.initial(data, None, None)
    }

    /// Prepare card faces from caller-owned 5:7 RGB565 artwork. The source is
    /// copied into the retained faces, so the caller may release it afterwards.
    /// Missing or malformed entries fall back to the generated card surface.
    pub fn prepare_initial_with_artwork(
        self,
        data: LauncherData<'_>,
        artwork: &[&[Rgb565Pixel]],
    ) -> InitialLauncher {
        self.initial(data, Some(Artwork::Rgb565(artwork)), None)
    }

    /// Prepare production chrome and card faces with the application's bitmap
    /// fonts. The fonts are consumed during preparation and are not retained.
    pub fn prepare_initial_with_artwork_and_typography(
        self,
        data: LauncherData<'_>,
        artwork: &[&[Rgb565Pixel]],
        typography: LauncherTypography<'_>,
    ) -> InitialLauncher {
        self.initial(data, Some(Artwork::Rgb565(artwork)), Some(typography))
    }

    /// Prepare native responsive faces from 360x504 RGB888 source artwork.
    /// Quantise only after destination-size filtering; scanout remains RGB565.
    pub fn prepare_initial_with_rgb888_artwork_and_typography(
        self,
        data: LauncherData<'_>,
        artwork: &[&[u8]],
        typography: LauncherTypography<'_>,
    ) -> InitialLauncher {
        self.initial(data, Some(Artwork::Rgb888(artwork)), Some(typography))
    }

    fn initial(
        self,
        data: LauncherData<'_>,
        artwork: Option<Artwork<'_>>,
        typography: Option<LauncherTypography<'_>>,
    ) -> InitialLauncher {
        let mut prepared = PreparedLauncher::new(self, data, artwork, typography);
        prepared.render_frame(BrowseFrame {
            selected: data.selected,
            target: data.selected,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        });
        InitialLauncher { prepared }
    }
}

#[derive(Clone, Copy)]
enum Artwork<'a> {
    Rgb565(&'a [&'a [Rgb565Pixel]]),
    Rgb888(&'a [&'a [u8]]),
}

pub struct InitialLauncher {
    prepared: PreparedLauncher,
}
impl InitialLauncher {
    pub fn pixels(&self) -> &[Rgb565Pixel] {
        self.prepared.pixels()
    }
    pub fn finish(self) -> PreparedLauncher {
        self.prepared
    }
}

/// Reusable launcher composition. All owned strings and working buffers are
/// created during preparation; `render_into` is allocation-free.
pub struct PreparedLauncher {
    scene: LauncherScene,
    responsive: Option<responsive::Layout>,
    logical: Vec<Rgb565Pixel>,
    fitted: Vec<Rgb565Pixel>,
    faces: Arc<Vec<CardFaces>>,
    flip_columns: Vec<crate::launcher_flip::Scratch>,
}

struct CardFaces {
    compact: crate::launcher_flip::Face,
    detail: crate::launcher_flip::Face,
}

/// Exact state represented by a prepared buffer. Consumers own the clock and
/// generation; preparing a frame never advances navigation or reads input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LauncherFrameRequest {
    pub frame: BrowseFrame,
    pub timestamp_us: u64,
    pub generation: u64,
}

/// Exclusive working-buffer ownership can move between threads. There is no
/// framebuffer, runtime or mutable shared texture state in this object.
pub struct PreparedLauncherFrame {
    request: Option<LauncherFrameRequest>,
    scratch: Vec<crate::launcher_flip::Scratch>,
    pixels: Vec<Rgb565Pixel>,
    blocked: Vec<Rgb565Pixel>,
    clip: (usize, usize),
}

impl PreparedLauncherFrame {
    pub fn pixels(&self) -> &[Rgb565Pixel] {
        &self.pixels
    }
    pub fn clip(&self) -> (usize, usize) {
        self.clip
    }
    pub fn request(&self) -> Option<LauncherFrameRequest> {
        self.request
    }
    pub fn storage_bytes(&self) -> usize {
        self.pixels.capacity() * 2
            + self.blocked.capacity() * 2
            + self
                .scratch
                .iter()
                .map(crate::launcher_flip::Scratch::storage_bytes)
                .sum::<usize>()
    }
}

#[derive(Clone)]
pub struct LauncherFramePreparer {
    faces: Arc<Vec<CardFaces>>,
}

impl LauncherFramePreparer {
    pub fn render_tile(
        &self,
        request: LauncherFrameRequest,
        buffer: &mut PreparedLauncherFrame,
        clip: (usize, usize),
    ) {
        let PreparedLauncherFrame {
            request: rendered_request,
            scratch,
            pixels,
            clip: rendered_clip,
            ..
        } = buffer;
        *rendered_request = Some(request);
        *rendered_clip = clip;
        self.render_tile_pixels(request, scratch, pixels, clip);
    }

    /// Render a tile directly into a native 960x540 destination while keeping
    /// the reusable projection scratch in the caller-owned tile buffer.
    pub fn render_tile_into(
        &self,
        request: LauncherFrameRequest,
        buffer: &mut PreparedLauncherFrame,
        destination: &mut [Rgb565Pixel],
        clip: (usize, usize),
        retain_pixels: bool,
    ) {
        assert!(destination.len() >= 960 * 540);
        buffer.request = Some(request);
        buffer.clip = clip;
        self.render_tile_pixels(request, &mut buffer.scratch, destination, clip);
        if retain_pixels {
            for y in 120..495 {
                buffer.pixels[y * 960 + clip.0..y * 960 + clip.1]
                    .copy_from_slice(&destination[y * 960 + clip.0..y * 960 + clip.1]);
            }
        }
    }

    /// Compose each screen strip in cached memory, then publish it to the
    /// write-combined scanout mapping with contiguous row stores.
    pub fn render_tile_blocked_into(
        &self,
        request: LauncherFrameRequest,
        buffer: &mut PreparedLauncherFrame,
        destination: &mut [Rgb565Pixel],
        clip: (usize, usize),
        retain_pixels: bool,
    ) {
        const TOP: usize = 120;
        const BOTTOM: usize = 495;
        assert!(destination.len() >= LOGICAL_WIDTH * LOGICAL_HEIGHT);
        assert!(clip.0 >= 296 && clip.0 <= clip.1 && clip.1 <= 934);
        buffer.request = Some(request);
        buffer.clip = clip;
        self.render_tile_blocked_pixels(
            request,
            &mut buffer.scratch,
            &mut buffer.blocked,
            destination,
            clip,
        );
        if retain_pixels {
            for y in TOP..BOTTOM {
                buffer.pixels[y * LOGICAL_WIDTH + clip.0..y * LOGICAL_WIDTH + clip.1]
                    .copy_from_slice(
                        &destination[y * LOGICAL_WIDTH + clip.0..y * LOGICAL_WIDTH + clip.1],
                    );
            }
        }
    }

    fn render_tile_blocked_pixels(
        &self,
        request: LauncherFrameRequest,
        scratch: &mut [crate::launcher_flip::Scratch],
        blocked: &mut [Rgb565Pixel],
        destination: &mut [Rgb565Pixel],
        clip: (usize, usize),
    ) {
        const TOP: usize = 120;
        const BOTTOM: usize = 495;
        if !self.faces.is_empty() {
            let plan = build_carousel_plan(&self.faces, request.frame);
            let width = crate::launcher_flip::STRIP_WIDTH;
            for left in (clip.0..clip.1).step_by(width) {
                let right = (left + width).min(clip.1);
                let block_width = right - left;
                let block_len = block_width * (BOTTOM - TOP);
                let block = &mut blocked[..block_len];
                block.fill(Rgb565Pixel(BACKGROUND));
                draw_carousel_plan(
                    block,
                    block_width,
                    (left, TOP),
                    &plan,
                    scratch,
                    (left, right),
                );
                for y in TOP..BOTTOM {
                    let source = (y - TOP) * block_width;
                    destination[y * LOGICAL_WIDTH + left..y * LOGICAL_WIDTH + right]
                        .copy_from_slice(&block[source..source + block_width]);
                }
            }
        } else {
            for y in TOP..BOTTOM {
                destination[y * LOGICAL_WIDTH + clip.0..y * LOGICAL_WIDTH + clip.1]
                    .fill(Rgb565Pixel(BACKGROUND));
            }
        }
    }

    fn render_tile_pixels(
        &self,
        request: LauncherFrameRequest,
        scratch: &mut [crate::launcher_flip::Scratch],
        pixels: &mut [Rgb565Pixel],
        clip: (usize, usize),
    ) {
        assert!(clip.0 >= 296 && clip.0 <= clip.1 && clip.1 <= 934);
        for y in 120..495 {
            pixels[y * 960 + clip.0..y * 960 + clip.1].fill(Rgb565Pixel(0));
        }
        if !self.faces.is_empty() {
            let plan = build_carousel_plan(&self.faces, request.frame);
            // Each screen strip is independent: finish every reflection before
            // its bodies, then reuse the same cache-local scratch for the next.
            let width = crate::launcher_flip::STRIP_WIDTH;
            for left in (clip.0..clip.1).step_by(width) {
                draw_carousel_plan(
                    pixels,
                    LOGICAL_WIDTH,
                    (0, 0),
                    &plan,
                    scratch,
                    (left, (left + width).min(clip.1)),
                );
            }
        }
    }
    /// Compact scratch for `render_tile` only, not whole-card preparation.
    pub fn new_tile_buffer(&self) -> PreparedLauncherFrame {
        PreparedLauncherFrame {
            request: None,
            scratch: (0..6)
                .map(|_| crate::launcher_flip::Scratch::strip())
                .collect(),
            pixels: vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT],
            blocked: vec![Rgb565Pixel(BACKGROUND); crate::launcher_flip::STRIP_WIDTH * (495 - 120)],
            clip: (296, 934),
        }
    }
}

fn bake_face(
    card: &PreparedCard<'_>,
    width: usize,
    selected: bool,
    typography: Option<LauncherTypography<'_>>,
) -> crate::launcher_flip::Face {
    artwork::face(card, width, selected, typography)
}

struct PreparedCard<'a> {
    id: LauncherCardId,
    name: &'a str,
    games: Option<u32>,
    colour: u16,
    name_mask: Vec<[u8; 7]>,
    games_mask: Vec<[u8; 7]>,
    artwork: Option<&'a [Rgb565Pixel]>,
    rgb888: Option<&'a [u8]>,
}

impl PreparedLauncher {
    /// Refresh only static chrome. Callers must rebuild for changed card data,
    /// artwork, output geometry, or typography. Faces and scratch stay resident.
    pub fn refresh_chrome(
        &mut self,
        data: LauncherData<'_>,
        typography: Option<LauncherTypography<'_>>,
    ) {
        if let Some(layout) = &self.responsive {
            layout.chrome(&mut self.logical, data, &layout.fonts(typography));
        } else {
            render_logical(&mut self.logical, data, typography);
        }
        self.fit_output();
    }

    pub fn compose_tiles(&mut self, left: &PreparedLauncherFrame, right: &PreparedLauncherFrame) {
        assert_eq!(left.request, right.request);
        assert_eq!(left.clip.0, 296);
        assert_eq!(left.clip.1, right.clip.0);
        assert_eq!(right.clip.1, 934);
        let split = left.clip.1;
        left.request.expect("rendered tile");
        for y in 120..495 {
            self.logical[y * 960 + 296..y * 960 + split]
                .copy_from_slice(&left.pixels[y * 960 + 296..y * 960 + split]);
            self.logical[y * 960 + split..y * 960 + 934]
                .copy_from_slice(&right.pixels[y * 960 + split..y * 960 + 934]);
        }
        self.fit_output();
    }
    pub fn frame_preparer(&self) -> LauncherFramePreparer {
        LauncherFramePreparer {
            faces: self.faces.clone(),
        }
    }
    /// Owned raster-buffer capacity, excluding strings and small metadata.
    /// This is not process RSS; it makes the quality/cache tradeoff measurable.
    pub fn cached_raster_bytes(&self) -> usize {
        (self.logical.capacity() + self.fitted.capacity()) * 2
            + self
                .faces
                .iter()
                .map(|f| f.compact.storage_bytes() + f.detail.storage_bytes())
                .sum::<usize>()
            + self
                .flip_columns
                .iter()
                .map(crate::launcher_flip::Scratch::storage_bytes)
                .sum::<usize>()
    }

    fn new(
        scene: LauncherScene,
        data: LauncherData<'_>,
        artwork: Option<Artwork<'_>>,
        typography: Option<LauncherTypography<'_>>,
    ) -> Self {
        let cards = data
            .cards
            .iter()
            .enumerate()
            .map(|(index, card)| PreparedCard {
                id: card.id,
                name: card.name,
                games: card.games,
                colour: card.colour,
                name_mask: text_mask(card.name),
                games_mask: card
                    .games
                    .map_or_else(Vec::new, |games| text_mask(&format_games(games))),
                artwork: artwork
                    .and_then(|items| match items {
                        Artwork::Rgb565(items) => items.get(index),
                        Artwork::Rgb888(_) => None,
                    })
                    .filter(|pixels| pixels.len() == 180 * card_height(180))
                    .copied(),
                rgb888: artwork
                    .and_then(|items| match items {
                        Artwork::Rgb888(items) => items.get(index),
                        Artwork::Rgb565(_) => None,
                    })
                    .filter(|pixels| pixels.len() == 360 * 504 * 3)
                    .copied(),
            });
        let responsive = responsive::Layout::for_scene(scene);
        let fonts = responsive.map(|layout| layout.fonts(typography));
        let pixel_count = if responsive.is_some() {
            scene.width * scene.height
        } else {
            LOGICAL_WIDTH * LOGICAL_HEIGHT
        };
        let mut chrome = vec![Rgb565Pixel(BACKGROUND); pixel_count];
        if let Some((layout, fonts)) = responsive.as_ref().zip(fonts.as_ref()) {
            layout.chrome(&mut chrome, data, fonts);
        } else {
            render_logical(&mut chrome, data, typography);
        }
        let faces: Vec<_> = cards
            .map(|card| {
                if let Some((layout, fonts)) = responsive.as_ref().zip(fonts.as_ref()) {
                    layout.faces(&card, fonts)
                } else {
                    CardFaces {
                        compact: bake_face(&card, 180, false, typography),
                        detail: bake_face(&card, 180, true, typography),
                    }
                }
            })
            .collect();
        Self {
            scene,
            responsive,
            logical: chrome,
            fitted: if responsive.is_some()
                || (scene.width == LOGICAL_WIDTH && scene.height == LOGICAL_HEIGHT)
            {
                Vec::new()
            } else {
                vec![Rgb565Pixel(BACKGROUND); scene.width * scene.height]
            },
            faces: Arc::new(faces),
            flip_columns: (0..6)
                .map(|_| {
                    if let Some(layout) = responsive {
                        crate::launcher_flip::Scratch::sized(
                            crate::launcher_flip::STRIP_WIDTH,
                            scene.width,
                            layout.card_h,
                            scene.height,
                        )
                    } else {
                        crate::launcher_flip::Scratch::new()
                    }
                })
                .collect(),
        }
    }

    pub fn render_into(&mut self, frame: BrowseFrame, output: &mut [Rgb565Pixel]) {
        self.render_frame(frame);
        output.copy_from_slice(self.pixels());
    }

    /// Compose into the retained buffer. Native-size consumers can borrow it
    /// directly rather than copying through an intermediate fitted surface.
    pub fn render_frame(&mut self, frame: BrowseFrame) {
        if let Some(layout) = self.responsive {
            layout.render(
                &mut self.logical,
                &self.faces,
                frame,
                &mut self.flip_columns,
            );
            return;
        }
        #[cfg(feature = "launcher-profile")]
        let clear_profile = crate::launcher_profile::span("scene.clear");
        // All animation, including projected edges and reflections, is clipped
        // to this region. Keep static chrome resident between frames.
        for rect in Self::logical_damage() {
            for y in rect.y0..rect.y1 {
                let range = y * LOGICAL_WIDTH + rect.x0..y * LOGICAL_WIDTH + rect.x1;
                // The damage region contains only the pure-black background in
                // chrome. Avoid reading a second framebuffer just to clear.
                self.logical[range].fill(Rgb565Pixel(BACKGROUND));
            }
        }
        #[cfg(feature = "launcher-profile")]
        drop(clear_profile);
        if self.faces.is_empty() {
            self.fit_output();
            return;
        }
        let plan = build_carousel_plan(&self.faces, frame);
        for left in (296..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
            draw_carousel_plan(
                &mut self.logical,
                LOGICAL_WIDTH,
                (0, 0),
                &plan,
                &mut self.flip_columns,
                (left, (left + crate::launcher_flip::STRIP_WIDTH).min(934)),
            );
        }
        // The projected card rasterizer clips its writes to the
        // carousel. Static margins therefore need no restoration pass.
        self.fit_output();
    }

    fn fit_output(&mut self) {
        if self.responsive.is_none()
            && (self.scene.width != LOGICAL_WIDTH || self.scene.height != LOGICAL_HEIGHT)
        {
            scale_into(
                &self.logical,
                self.scene.width,
                self.scene.height,
                &mut self.fitted,
            );
        }
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        if self.responsive.is_some()
            || (self.scene.width == LOGICAL_WIDTH && self.scene.height == LOGICAL_HEIGHT)
        {
            &self.logical
        } else {
            &self.fitted
        }
    }

    const fn logical_damage() -> [crate::Rgb565Rect; 1] {
        [crate::Rgb565Rect {
            x0: 296,
            y0: 120,
            x1: 934,
            y1: 495,
        }]
    }

    /// Conservative union of every previous/current card pose and reflection.
    /// At other output sizes, fitting still invalidates the complete surface.
    pub fn damage(&self) -> [crate::Rgb565Rect; 1] {
        if self.scene.width == LOGICAL_WIDTH && self.scene.height == LOGICAL_HEIGHT {
            Self::logical_damage()
        } else {
            [crate::Rgb565Rect {
                x0: 0,
                y0: 0,
                x1: self.scene.width,
                y1: self.scene.height,
            }]
        }
    }
}

const BACKGROUND: u16 = rgb(0, 0, 0);
const CREAM: u16 = rgb(238, 232, 213);
const MUTED: u16 = rgb(143, 151, 150);
const RULE: u16 = rgb(48, 61, 63);

pub(super) const fn card_height(width: usize) -> usize {
    width * 7 / 5
}

fn slot_geometry(relative: isize) -> (i32, i32) {
    match relative {
        // Hidden return slots sit inside the outer visible card, not beyond
        // the screen edge. Smaller silhouettes (and their reflections) are
        // covered by that neighbour until it moves toward the centre.
        ..=-3 => (356, 36),
        -2 => (356, 56),
        -1 => (466, 72),
        0 => (610, 90),
        1 => (754, 72),
        2 => (864, 56),
        _ => (864, 36),
    }
}

const fn rgb(red: u16, green: u16, blue: u16) -> u16 {
    ((red >> 3) << 11) | ((green >> 2) << 5) | (blue >> 3)
}

fn render_logical(
    pixels: &mut [Rgb565Pixel],
    data: LauncherData<'_>,
    typography: Option<LauncherTypography<'_>>,
) {
    draw_rect(pixels, 0, 0, LOGICAL_WIDTH, LOGICAL_HEIGHT, BACKGROUND);
    draw_role_text(
        pixels,
        typography,
        TextRole::Heading,
        26,
        20,
        "MISTER MAGIK",
        CREAM,
        3,
    );
    draw_role_text(
        pixels,
        typography,
        TextRole::Heading,
        875,
        22,
        data.clock,
        CREAM,
        2,
    );
    draw_line(pixels, 26, 76, 934, 76, RULE);

    draw_line(pixels, 265, 95, 265, 478, RULE);
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        29,
        101,
        "YOUR LIBRARY",
        MUTED,
        1,
    );
    draw_role_number(pixels, typography, 28, 142, data.library_games, CREAM, 5);
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        29,
        205,
        "GAMES READY TO PLAY",
        MUTED,
        1,
    );
    draw_line(pixels, 28, 239, 240, 239, RULE);
    draw_role_number(pixels, typography, 30, 265, data.collections, CREAM, 3);
    draw_role_number(pixels, typography, 150, 265, data.favourites, CREAM, 3);
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        30,
        310,
        "COLLECTIONS",
        MUTED,
        1,
    );
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        150,
        310,
        "FAVOURITES",
        MUTED,
        1,
    );
    draw_line(pixels, 28, 340, 240, 340, RULE);
    for (index, colour) in [
        rgb(226, 52, 67),
        rgb(237, 193, 54),
        rgb(85, 170, 91),
        rgb(41, 145, 196),
    ]
    .iter()
    .enumerate()
    {
        draw_rect(pixels, 29 + index * 54, 436, 48, 7, *colour);
    }

    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        296,
        101,
        "COLLECTIONS",
        MUTED,
        1,
    );
    draw_line(pixels, 26, 500, 934, 500, RULE);
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        30,
        516,
        "A  OPEN",
        CREAM,
        1,
    );
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        130,
        516,
        "B  BACK",
        CREAM,
        1,
    );
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        586,
        516,
        "←  →   BROWSE CARDS",
        CREAM,
        1,
    );
}

const GEOMETRY_ONE: i64 = 65536;

fn slot_angle(relative: isize) -> i64 {
    // Mirrored, gently receding faces: 6 degrees per slot, capped at 18.
    relative.clamp(-3, 3) as i64 * GEOMETRY_ONE / 30
}

fn continuous_geometry(
    relative: isize,
    destination: isize,
    progress: i64,
) -> crate::launcher_flip::Pose {
    let (start_centre, start_scale) = slot_geometry(relative);
    let (end_centre, end_scale) = slot_geometry(destination);
    let centre =
        i64::from(start_centre) * GEOMETRY_ONE + i64::from(end_centre - start_centre) * progress;
    let width =
        2 * (i64::from(start_scale) * GEOMETRY_ONE + i64::from(end_scale - start_scale) * progress);
    let height = width * 7 / 5;
    crate::launcher_flip::Pose {
        x: centre - width / 2,
        top: 284 * GEOMETRY_ONE - height / 2,
        width,
        height,
        angle: slot_angle(relative)
            + (slot_angle(destination) - slot_angle(relative)) * progress / GEOMETRY_ONE,
        clip: (296, 934),
        body_clip: (296, 934),
        vertical_clip: (120, 438, 495),
    }
}

fn smooth_progress(progress: u32, duration: u32) -> i64 {
    let d = i64::from(duration.max(1));
    let t = i64::from(progress.min(duration));
    if duration != crate::launcher_navigation::SPRING_POSITION_UNITS {
        i64::from(crate::spring_animation::smooth_spring_q16(
            (t * i64::from(u16::MAX) / d) as u16,
        )) * GEOMETRY_ONE
            / i64::from(u16::MAX)
    } else {
        t * GEOMETRY_ONE / d
    }
}

fn ease_in_out_sine(progress: i64) -> i64 {
    let (_, cosine) = crate::launcher_flip::sin_cos(progress.clamp(0, GEOMETRY_ONE));
    (GEOMETRY_ONE - cosine) / 2
}

fn flip_spin(right: bool) -> i64 {
    if right { -1 } else { 1 }
}

#[derive(Clone, Copy)]
struct CarouselItem<'a> {
    face: &'a crate::launcher_flip::Face,
    blend: Option<(&'a crate::launcher_flip::Face, u32)>,
    pose: crate::launcher_flip::Pose,
}

struct CarouselPlan<'a> {
    items: [Option<CarouselItem<'a>>; 6],
}

fn build_carousel_plan<'a>(faces: &'a [CardFaces], mut motion: BrowseFrame) -> CarouselPlan<'a> {
    let settled = motion.phase == crate::launcher_navigation::BrowsePhase::Settled;
    if !settled && (motion.progress_millis == 0 || motion.progress_millis >= motion.duration_millis)
    {
        motion.selected = if motion.progress_millis == 0 {
            motion.selected
        } else {
            motion.target
        };
        motion.phase = crate::launcher_navigation::BrowsePhase::Settled;
    }
    let settled = motion.phase == crate::launcher_navigation::BrowsePhase::Settled;
    let selected = motion.selected % faces.len();
    let progress = if settled {
        0
    } else {
        smooth_progress(motion.progress_millis, motion.duration_millis)
    };
    let right = motion.direction == Some(BrowseDirection::Right);
    let relatives: &[isize] = if settled {
        &[2, -2, 1, -1, 0]
    } else if progress * 2 > GEOMETRY_ONE {
        if right {
            &[-2, 3, -1, 2, 0, 1]
        } else {
            &[2, -3, 1, -2, 0, -1]
        }
    } else if right {
        &[3, -2, 2, -1, 1, 0]
    } else {
        &[-3, 2, -2, 1, -1, 0]
    };
    let mut items = [None; 6];
    for (slot, relative) in relatives.iter().enumerate() {
        let index = (selected as isize + *relative).rem_euclid(faces.len() as isize) as usize;
        let destination = if settled {
            *relative
        } else if right {
            *relative - 1
        } else {
            *relative + 1
        };
        let mut pose = continuous_geometry(*relative, destination, progress);
        let side = relative.signum();
        if side != 0 && destination.signum() == side && settled {
            let cover = continuous_geometry(relative - side, destination - side, progress);
            let (sin, cos) = crate::launcher_flip::sin_cos(cover.angle);
            let local = side as i64 * cover.width / 2;
            let depth = GEOMETRY_ONE + local * sin / (cover.width * 4);
            let edge = (cover.x + cover.width / 2 + local * cos / depth) / GEOMETRY_ONE;
            if side > 0 {
                pose.body_clip.0 = ((edge - 8).max(0) as usize).clamp(296, 934);
            } else {
                pose.body_clip.1 = ((edge + 8).max(0) as usize).clamp(296, 934);
            }
        }
        let prominence = if *relative == 0 {
            256 - (256 * progress / GEOMETRY_ONE)
        } else if destination == 0 {
            256 * progress / GEOMETRY_ONE
        } else {
            0
        };
        let incoming = destination == 0 && *relative != 0;
        let flipping_card = (incoming || *relative == 0)
            && progress > 0
            && progress < GEOMETRY_ONE
            && motion.phase == crate::launcher_navigation::BrowsePhase::Flipping;
        let (face, blend) = if flipping_card {
            let outgoing = *relative == 0;
            let angle = pose.angle + flip_spin(right) * ease_in_out_sine(progress);
            let (_, cos) = crate::launcher_flip::sin_cos(angle);
            pose.angle = angle;
            (
                if (cos < 0) == outgoing {
                    &faces[index].compact
                } else {
                    &faces[index].detail
                },
                None,
            )
        } else {
            let face = if prominence == 256 {
                &faces[index].detail
            } else {
                &faces[index].compact
            };
            (
                face,
                (prominence > 0 && prominence < 256)
                    .then_some((&faces[index].detail, prominence as u32)),
            )
        };
        items[slot] = Some(CarouselItem { face, blend, pose });
    }
    CarouselPlan { items }
}

fn draw_carousel_plan(
    pixels: &mut [Rgb565Pixel],
    pitch: usize,
    origin: (usize, usize),
    plan: &CarouselPlan<'_>,
    scratch: &mut [crate::launcher_flip::Scratch],
    clip: (usize, usize),
) {
    for (slot, item) in plan.items.iter().enumerate() {
        let Some(item) = item else { continue };
        let mut pose = item.pose;
        pose.clip = clip;
        pose.body_clip.0 = pose.body_clip.0.max(clip.0).min(clip.1);
        pose.body_clip.1 = pose.body_clip.1.min(clip.1).max(clip.0);
        crate::launcher_flip::draw_target(
            pixels,
            pitch,
            origin,
            item.face,
            pose,
            &mut scratch[slot],
            artwork::reflection_colour,
            true,
            item.blend,
        );
    }
    let mut covered = crate::launcher_flip::BodyOcclusion::new(clip);
    let mut occlusion = [covered; 6];
    for (slot, item) in plan.items.iter().enumerate().rev() {
        occlusion[slot] = covered;
        let Some(item) = item else { continue };
        let mut pose = item.pose;
        pose.clip = clip;
        pose.body_clip.0 = pose.body_clip.0.max(clip.0).min(clip.1);
        pose.body_clip.1 = pose.body_clip.1.min(clip.1).max(clip.0);
        crate::launcher_flip::add_opaque_coverage(item.face, pose, &scratch[slot], &mut covered);
    }
    for (slot, item) in plan.items.iter().enumerate() {
        let Some(item) = item else { continue };
        let mut pose = item.pose;
        pose.clip = clip;
        pose.body_clip.0 = pose.body_clip.0.max(clip.0).min(clip.1);
        pose.body_clip.1 = pose.body_clip.1.min(clip.1).max(clip.0);
        crate::launcher_flip::draw_occluded_target(
            pixels,
            pitch,
            origin,
            item.face,
            pose,
            &mut scratch[slot],
            artwork::reflection_colour,
            item.blend,
            &occlusion[slot],
        );
    }
}

pub(super) fn mix_colour(background: u16, foreground: u16, amount: usize) -> u16 {
    let amount = amount.min(256) as u32;
    let channel = |shift: u32, bits: u32| {
        let mask = (1_u16 << bits) - 1;
        let a = u32::from((background >> shift) & mask);
        let b = u32::from((foreground >> shift) & mask);
        ((a * (256 - amount) + b * amount) / 256) as u16
    };
    (channel(11, 5) << 11) | (channel(5, 6) << 5) | channel(0, 5)
}

fn format_games(games: u32) -> String {
    format!("{} GAMES", games)
}

fn text_mask(text: &str) -> Vec<[u8; 7]> {
    text.chars().map(glyph).collect()
}

pub(super) fn rounded_contains(x: usize, y: usize, width: usize, height: usize) -> bool {
    if x >= width || y >= height {
        return false;
    }
    let row = y.min(height - 1 - y);
    let inset = [5, 3, 2, 1, 1, 0, 0, 0].get(row).copied().unwrap_or(0);
    x >= inset && x + inset < width
}

fn scale_into(logical: &[Rgb565Pixel], width: usize, height: usize, output: &mut [Rgb565Pixel]) {
    if width == 0 || height == 0 || output.len() < width.saturating_mul(height) {
        return;
    }
    if width == LOGICAL_WIDTH && height == LOGICAL_HEIGHT {
        output[..LOGICAL_WIDTH * LOGICAL_HEIGHT].copy_from_slice(logical);
        return;
    }
    output.fill(Rgb565Pixel(BACKGROUND));
    let (scaled_width, scaled_height) =
        if width.saturating_mul(LOGICAL_HEIGHT) <= height.saturating_mul(LOGICAL_WIDTH) {
            (width, width * LOGICAL_HEIGHT / LOGICAL_WIDTH)
        } else {
            (height * LOGICAL_WIDTH / LOGICAL_HEIGHT, height)
        };
    if scaled_width == 0 || scaled_height == 0 {
        return;
    }
    let x_offset = (width - scaled_width) / 2;
    let y_offset = (height - scaled_height) / 2;
    for y in 0..scaled_height {
        let source_y = y * LOGICAL_HEIGHT / scaled_height;
        for x in 0..scaled_width {
            let source_x = x * LOGICAL_WIDTH / scaled_width;
            output[(y + y_offset) * width + x + x_offset] =
                logical[source_y * LOGICAL_WIDTH + source_x];
        }
    }
}

fn draw_line(pixels: &mut [Rgb565Pixel], x0: usize, y0: usize, x1: usize, y1: usize, colour: u16) {
    if x0 == x1 {
        for y in y0..=y1 {
            set_pixel(pixels, x0, y, colour);
        }
    } else {
        for x in x0..=x1 {
            set_pixel(pixels, x, y0, colour);
        }
    }
}

fn draw_rect(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    colour: u16,
) {
    for row in y..y.saturating_add(height).min(LOGICAL_HEIGHT) {
        for column in x..x.saturating_add(width).min(LOGICAL_WIDTH) {
            set_pixel(pixels, column, row, colour);
        }
    }
}

fn draw_mask_scaled_centered(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    mask: &[[u8; 7]],
    colour: u16,
    scale_q8: usize,
) {
    let text_width = mask.len() * 6 * scale_q8 / 256;
    let origin = x + width.saturating_sub(text_width) / 2;
    for (index, glyph) in mask.iter().enumerate() {
        let glyph_x = origin + index * 6 * scale_q8 / 256;
        for (row, bits) in glyph.iter().enumerate() {
            let y0 = y + row * scale_q8 / 256;
            let y1 = y + (row + 1) * scale_q8 / 256;
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    let x0 = glyph_x + column * scale_q8 / 256;
                    let x1 = glyph_x + (column + 1) * scale_q8 / 256;
                    draw_rect(pixels, x0, y0, (x1 - x0).max(1), (y1 - y0).max(1), colour);
                }
            }
        }
    }
}

fn draw_number(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    value: u32,
    colour: u16,
    scale: usize,
) {
    draw_text(pixels, x, y, &value.to_string(), colour, scale);
}

fn draw_scene_text(
    pixels: &mut [Rgb565Pixel],
    font: Option<&BitmapFont>,
    x: usize,
    y: usize,
    text: &str,
    colour: u16,
    legacy_scale: usize,
) {
    if let Some(font) = font {
        font.draw(
            pixels,
            LOGICAL_WIDTH,
            LOGICAL_HEIGHT,
            x as i32,
            y as i32,
            text,
            colour,
        );
    } else {
        draw_text(pixels, x, y, text, colour, legacy_scale);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_role_text(
    pixels: &mut [Rgb565Pixel],
    typography: Option<LauncherTypography<'_>>,
    role: TextRole,
    x: usize,
    y: usize,
    text: &str,
    colour: u16,
    legacy_scale: usize,
) {
    if let Some(fonts) = typography {
        draw_scene_text(
            pixels,
            Some(fonts.font_for(role, text)),
            x,
            y,
            text,
            colour,
            legacy_scale,
        );
    } else {
        draw_scene_text(pixels, None, x, y, text, colour, legacy_scale);
    }
}

fn draw_role_number(
    pixels: &mut [Rgb565Pixel],
    typography: Option<LauncherTypography<'_>>,
    x: usize,
    y: usize,
    value: u32,
    colour: u16,
    legacy_scale: usize,
) {
    let text = value.to_string();
    if let Some(fonts) = typography {
        let font = fonts.font_for(TextRole::Number, &text);
        draw_scene_text(pixels, Some(font), x, y, &text, colour, legacy_scale);
    } else {
        draw_number(pixels, x, y, value, colour, legacy_scale);
    }
}

fn draw_text(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    text: &str,
    colour: u16,
    scale: usize,
) {
    let mut cursor = x;
    for character in text.chars() {
        draw_glyph(pixels, cursor, y, character, colour, scale);
        cursor += 6 * scale;
    }
}

fn draw_glyph(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    character: char,
    colour: u16,
    scale: usize,
) {
    let glyph = glyph(character);
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) != 0 {
                draw_rect(
                    pixels,
                    x + column * scale,
                    y + row * scale,
                    scale,
                    scale,
                    colour,
                );
            }
        }
    }
}

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [15, 16, 16, 16, 16, 16, 15],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [15, 16, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [31, 4, 4, 4, 4, 4, 31],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 17, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 27, 17],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ':' => [0, 6, 6, 0, 6, 6, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '←' => [4, 2, 31, 2, 4, 0, 0],
        '→' => [4, 8, 31, 8, 4, 0, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 6, 6],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

fn set_pixel(pixels: &mut [Rgb565Pixel], x: usize, y: usize, colour: u16) {
    if x < LOGICAL_WIDTH && y < LOGICAL_HEIGHT {
        pixels[y * LOGICAL_WIDTH + x] = Rgb565Pixel(colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: [LauncherCard<'static>; 5] = [
        LauncherCard {
            id: LauncherCardId::Arcade,
            name: "ARCADE",
            games: Some(1752),
            colour: rgb(142, 27, 48),
        },
        LauncherCard {
            id: LauncherCardId::Consoles,
            name: "SNK",
            games: Some(324),
            colour: rgb(30, 75, 125),
        },
        LauncherCard {
            id: LauncherCardId::Consoles,
            name: "CONSOLES",
            games: Some(842),
            colour: rgb(199, 190, 167),
        },
        LauncherCard {
            id: LauncherCardId::Handhelds,
            name: "HANDHELDS",
            games: Some(126),
            colour: rgb(45, 90, 150),
        },
        LauncherCard {
            id: LauncherCardId::Computers,
            name: "COMPUTERS",
            games: Some(86),
            colour: rgb(190, 34, 55),
        },
    ];

    fn data() -> LauncherData<'static> {
        LauncherData {
            cards: &CARDS,
            selected: 0,
            library_games: 6842,
            collections: 18,
            favourites: 126,
            clock: "21:37",
        }
    }

    fn settled_frame(selected: usize) -> BrowseFrame {
        BrowseFrame {
            selected,
            target: selected,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        }
    }

    #[test]
    fn borrowed_native_frame_and_fitted_frame_match_reference() {
        for (width, height) in [(960, 540), (640, 480)] {
            let scene = LauncherScene::new(width, height);
            let mut prepared = PreparedLauncher::new(scene, data(), None, None);
            let frame = settled_frame(0);
            prepared.render_frame(frame);
            assert_eq!(prepared.pixels(), scene.render(data()));
            if width == LOGICAL_WIDTH {
                assert!(prepared.fitted.is_empty());
                assert_eq!(prepared.pixels().as_ptr(), prepared.logical.as_ptr());
            }
        }
    }

    #[test]
    fn retained_damage_matches_full_clear_across_wraps_and_directions() {
        for (width, height) in [(960, 540), (640, 480)] {
            let scene = LauncherScene::new(width, height);
            let mut incremental = PreparedLauncher::new(scene, data(), None, None);
            let mut reference = PreparedLauncher::new(scene, data(), None, None);
            let chrome = reference.logical.clone();
            for rect in PreparedLauncher::logical_damage() {
                for y in rect.y0..rect.y1 {
                    assert!(
                        chrome[y * 960 + rect.x0..y * 960 + rect.x1]
                            .iter()
                            .all(|pixel| pixel.0 == BACKGROUND)
                    );
                }
            }
            let mut previous = vec![Rgb565Pixel(0); width * height];
            for direction in [BrowseDirection::Right, BrowseDirection::Left] {
                for selected in 0..5 {
                    for progress_millis in [0, 46, 115, 230, 345, 414, 460] {
                        let frame = BrowseFrame {
                            selected,
                            target: (selected
                                + if direction == BrowseDirection::Right {
                                    1
                                } else {
                                    4
                                })
                                % 5,
                            direction: Some(direction),
                            phase: crate::launcher_navigation::BrowsePhase::Flipping,
                            progress_millis,
                            duration_millis: 460,
                        };
                        reference.logical.copy_from_slice(&chrome);
                        reference.render_frame(frame);
                        incremental.render_frame(frame);
                        assert_eq!(incremental.pixels(), reference.pixels());
                        for y in 120..495 {
                            for range in [y * 960..y * 960 + 296, y * 960 + 934..(y + 1) * 960] {
                                assert_eq!(incremental.logical[range.clone()], chrome[range]);
                            }
                        }
                        // First frame initializes static chrome in every slot.
                        if previous.iter().all(|p| p.0 == 0) {
                            previous.copy_from_slice(incremental.pixels());
                        } else {
                            for rect in incremental.damage() {
                                for y in rect.y0..rect.y1 {
                                    let range = y * width + rect.x0..y * width + rect.x1;
                                    previous[range.clone()]
                                        .copy_from_slice(&incremental.pixels()[range]);
                                }
                            }
                        }
                        assert_eq!(previous, reference.pixels());
                    }
                }
            }
        }
    }

    #[test]
    fn simplified_chrome_leaves_removed_copy_regions_pure_black() {
        let frame = LauncherScene::new(960, 540).render(data());
        assert_eq!(BACKGROUND, 0);
        for (left, top, right, bottom) in [
            (26, 53, 934, 60),
            (29, 366, 240, 407),
            (29, 459, 240, 466),
            (846, 516, 934, 523),
        ] {
            for y in top..bottom {
                assert!(
                    frame[y * 960 + left..y * 960 + right]
                        .iter()
                        .all(|pixel| pixel.0 == 0)
                );
            }
        }
    }

    #[test]
    fn output_is_deterministic_and_packed() {
        let scene = LauncherScene::new(960, 540);
        assert_eq!(scene.render(data()), scene.render(data()));
        assert_eq!(scene.render(data()).len(), 960 * 540);
    }

    #[test]
    fn fits_requested_sizes_with_exact_letterbox() {
        let frame = LauncherScene::new(640, 480).render(data());
        assert_eq!(frame.len(), 640 * 480);
        assert!(frame[..640 * 60].iter().all(|pixel| pixel.0 == BACKGROUND));
        assert!(frame[420 * 640..].iter().all(|pixel| pixel.0 == BACKGROUND));
        assert!(
            frame[60 * 640..420 * 640]
                .iter()
                .any(|pixel| pixel.0 != BACKGROUND)
        );
        assert_eq!(LauncherScene::new(1, 1).render(data()).len(), 1);
        assert!(LauncherScene::new(0, 0).render(data()).is_empty());
    }

    #[test]
    fn continuous_geometry_preserves_ratio_and_fractional_steps() {
        assert_eq!(slot_angle(0), 0);
        assert_eq!(slot_angle(-2), -slot_angle(2));
        assert!(slot_angle(2) > slot_angle(1));
        assert_eq!(slot_angle(4), slot_angle(3));
        for relative in -3..=3 {
            let destination = relative + 1;
            assert_eq!(
                continuous_geometry(relative, destination, 0).angle,
                slot_angle(relative)
            );
            assert_eq!(
                continuous_geometry(relative, destination, GEOMETRY_ONE).angle,
                slot_angle(destination)
            );
            let half = continuous_geometry(relative, destination, GEOMETRY_ONE / 2).angle;
            assert!(half >= slot_angle(relative) && half <= slot_angle(destination));
        }
        for direction in [-1, 1] {
            for relative in -2..=2 {
                for progress in 0..=460 {
                    let p = continuous_geometry(
                        relative,
                        relative + direction,
                        smooth_progress(progress, 460),
                    );
                    assert!((p.height * 5 - p.width * 7).abs() < 5);
                    assert_eq!(p.top + p.height / 2, 284 * GEOMETRY_ONE);
                }
            }
        }
        let a = continuous_geometry(1, 0, smooth_progress(100, 460));
        let b = continuous_geometry(1, 0, smooth_progress(101, 460));
        assert!(b.width > a.width && b.width - a.width < GEOMETRY_ONE);
        assert_ne!(a.x % GEOMETRY_ONE, 0);
        let end = continuous_geometry(1, 0, GEOMETRY_ONE);
        assert_eq!(
            (end.width, end.height),
            (180 * GEOMETRY_ONE, 252 * GEOMETRY_ONE)
        );
    }

    #[test]
    fn one_aspect_ratio_and_shared_progress_at_every_depth() {
        for direction in [-1, 1] {
            for relative in -2..=2 {
                let start = continuous_geometry(relative, relative + direction, 0);
                for progress in 0..=460 {
                    let pose = continuous_geometry(
                        relative,
                        relative + direction,
                        progress * GEOMETRY_ONE / 460,
                    );
                    assert!((pose.height * 5 - pose.width * 7).abs() < 5);
                    if progress == 115 {
                        assert_ne!(pose.x, start.x, "every card must already be moving");
                        if relative == 0 {
                            assert!(pose.width < start.width);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn return_slots_stay_inside_outer_neighbours_and_away_from_screen_edges() {
        for side in [-1, 1] {
            let visible = continuous_geometry(side * 2, side * 2, 0);
            let hidden = continuous_geometry(side * 3, side * 3, 0);
            assert!(hidden.x > visible.x);
            assert!(hidden.x + hidden.width < visible.x + visible.width);
            assert!(hidden.top > visible.top);
            assert!(hidden.top + hidden.height < visible.top + visible.height);
            for progress in 0..=460 {
                for (from, to) in [(side * 3, side * 2), (side * 2, side * 3)] {
                    let pose = continuous_geometry(from, to, progress * GEOMETRY_ONE / 460);
                    let (sin, cos) = crate::launcher_flip::sin_cos(pose.angle);
                    for local in [-pose.width / 2, pose.width / 2] {
                        let depth = GEOMETRY_ONE + local * sin / (pose.width * 4);
                        let edge = pose.x + pose.width / 2 + local * cos / depth;
                        assert!(edge > 296 * GEOMETRY_ONE);
                        assert!(edge < 934 * GEOMETRY_ONE);
                    }
                }
            }
        }
    }

    #[test]
    fn chrome_refresh_keeps_faces_and_scratch_and_matches_fresh_frame() {
        for (width, height) in [(960, 540), (540, 960)] {
            let scene = LauncherScene::new(width, height);
            let mut prepared = scene.prepare(data());
            let faces = prepared.faces.clone();
            let scratch = prepared.flip_columns.as_ptr();
            let mut updated = data();
            updated.clock = "22:00";
            updated.library_games += 123;
            updated.collections += 5;
            updated.favourites += 1;
            prepared.refresh_chrome(updated, None);
            assert!(Arc::ptr_eq(&prepared.faces, &faces));
            assert_eq!(prepared.flip_columns.as_ptr(), scratch);
            let frame = BrowseFrame {
                selected: 0,
                target: 1,
                phase: crate::launcher_navigation::BrowsePhase::Flipping,
                direction: Some(crate::launcher_navigation::BrowseDirection::Right),
                progress_millis: 91,
                duration_millis: 180,
            };
            prepared.render_frame(frame);
            let mut reference = scene.prepare(updated);
            reference.render_frame(frame);
            assert_eq!(prepared.pixels(), reference.pixels());
        }
    }

    #[test]
    #[ignore = "host microbenchmark; run explicitly with --release --ignored --nocapture"]
    fn chrome_refresh_benchmark() {
        use std::hint::black_box;
        use std::time::Instant;
        let scene = LauncherScene::new(960, 540);
        let mut prepared = scene.prepare(data());
        let faces = prepared.faces.clone();
        for sample in 0..3 {
            let mut updated = data();
            updated.clock = if sample % 2 == 0 { "21:38" } else { "21:37" };
            let full_started = Instant::now();
            let reference = black_box(scene.prepare(black_box(updated)));
            let full_us = full_started.elapsed().as_micros();
            let chrome_started = Instant::now();
            prepared.refresh_chrome(black_box(updated), None);
            black_box(prepared.pixels());
            let chrome_us = chrome_started.elapsed().as_micros();
            assert!(Arc::ptr_eq(&faces, &prepared.faces));
            let frame = settled_frame(0);
            prepared.render_frame(frame);
            assert_eq!(prepared.pixels(), reference.pixels());
            println!(
                "{{\"benchmark\":\"host-chrome-refresh\",\"sample\":{sample},\"full_prepare_us\":{full_us},\"chrome_refresh_us\":{chrome_us},\"rgb565_exact\":true}}"
            );
        }
    }

    #[test]
    fn prepared_resting_frame_matches_cold_render() {
        let scene = LauncherScene::new(960, 540);
        let mut prepared = scene.prepare(data());
        let mut output = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let frame = BrowseFrame {
            selected: 0,
            target: 0,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 180,
        };
        prepared.render_into(frame, &mut output);
        assert_eq!(output, scene.render(data()));
    }

    #[test]
    fn prepared_motion_changes_continuously_and_keeps_fixed_duration() {
        let scene = LauncherScene::new(960, 540);
        let mut prepared = scene.prepare(data());
        let mut start = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let mut middle = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let frame = |progress_millis| BrowseFrame {
            selected: 0,
            target: 1,
            phase: crate::launcher_navigation::BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis,
            duration_millis: 180,
        };
        prepared.render_into(frame(0), &mut start);
        prepared.render_into(frame(90), &mut middle);
        assert_ne!(start, middle);
        assert_eq!(frame(90).duration_millis, 180);
    }

    #[test]
    fn cold_motion_render_uses_the_prepared_renderer() {
        let scene = LauncherScene::new(960, 540);
        let frame = BrowseFrame {
            selected: 0,
            target: 1,
            phase: crate::launcher_navigation::BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: 90,
            duration_millis: 180,
        };
        let cold = scene.render_browse(data(), Some(frame));
        let mut prepared = scene.prepare(data());
        let mut hot = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        prepared.render_into(frame, &mut hot);
        assert_eq!(cold, hot);
    }

    #[test]
    fn tap_motion_uses_shared_spring_with_exact_endpoints() {
        assert_eq!(smooth_progress(0, 180), 0);
        for t in [1, 45, 90, 135, 179] {
            let expected = i64::from(crate::spring_animation::smooth_spring_q16(
                (t * u32::from(u16::MAX) / 180) as u16,
            )) * GEOMETRY_ONE
                / i64::from(u16::MAX);
            assert_eq!(smooth_progress(t, 180), expected);
        }
        assert_eq!(smooth_progress(180, 180), GEOMETRY_ONE);
    }

    #[test]
    fn position_driven_motion_eases_both_card_flips() {
        let prepared = LauncherScene::new(960, 540).prepare(data());
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        assert_eq!(ease_in_out_sine(0), 0);
        assert_eq!(ease_in_out_sine(GEOMETRY_ONE / 2), GEOMETRY_ONE / 2);
        assert_eq!(ease_in_out_sine(GEOMETRY_ONE), GEOMETRY_ONE);
        assert!(ease_in_out_sine(GEOMETRY_ONE / 4) < GEOMETRY_ONE / 4);
        assert!(ease_in_out_sine(3 * GEOMETRY_ONE / 4) > 3 * GEOMETRY_ONE / 4);
        for (direction, selected, target, relative) in [
            (BrowseDirection::Right, CARDS.len() - 1, 0, 1),
            (BrowseDirection::Left, 0, CARDS.len() - 1, -1),
        ] {
            for step in [1, units / 4, units / 2, 3 * units / 4, units - 1] {
                let motion = BrowseFrame {
                    selected,
                    target,
                    phase: crate::launcher_navigation::BrowsePhase::Flipping,
                    direction: Some(direction),
                    progress_millis: step,
                    duration_millis: units,
                };
                let plan = build_carousel_plan(&prepared.faces, motion);
                let (outgoing_slot, incoming_slot) = if step > units / 2 { (4, 5) } else { (5, 4) };
                let outgoing = plan.items[outgoing_slot].expect("outgoing card");
                let incoming = plan.items[incoming_slot].expect("incoming card");
                let progress = i64::from(step);
                let spin = flip_spin(direction == BrowseDirection::Right);
                let rotation = ease_in_out_sine(progress);
                assert_eq!(
                    outgoing.pose.angle,
                    continuous_geometry(0, -relative, progress).angle + spin * rotation,
                    "outgoing step {step} {direction:?}"
                );
                assert_eq!(
                    incoming.pose.angle,
                    continuous_geometry(relative, 0, progress).angle + spin * rotation,
                    "incoming step {step} {direction:?}"
                );
            }
            let settled = build_carousel_plan(
                &prepared.faces,
                BrowseFrame {
                    selected,
                    target,
                    phase: crate::launcher_navigation::BrowsePhase::Flipping,
                    direction: Some(direction),
                    progress_millis: units,
                    duration_millis: units,
                },
            );
            assert_eq!(settled.items[4].expect("settled top card").pose.angle, 0);
        }
    }

    #[test]
    fn held_motion_stays_linear_and_release_does_not_change_duration() {
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        assert_eq!(smooth_progress(0, units), 0);
        assert_eq!(smooth_progress(12345, units), 12345);
        assert_eq!(smooth_progress(units / 2, units), GEOMETRY_ONE / 2);
        assert_eq!(smooth_progress(units, units), GEOMETRY_ONE);
    }

    #[test]
    fn prepared_motion_has_pixel_identical_resting_endpoints() {
        let scene = LauncherScene::new(960, 540);
        let mut prepared = scene.prepare(data());
        let mut old = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let mut start = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let mut end = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let settled = |selected| BrowseFrame {
            selected,
            target: selected,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 180,
        };
        let moving = |progress_millis| BrowseFrame {
            selected: 0,
            target: 1,
            phase: crate::launcher_navigation::BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis,
            duration_millis: 180,
        };
        prepared.render_into(settled(0), &mut old);
        prepared.render_into(moving(0), &mut start);
        prepared.render_into(moving(180), &mut end);
        assert_eq!(start, old);
        prepared.render_into(settled(1), &mut old);
        for y in 120..495 {
            assert_eq!(
                &end[y * LOGICAL_WIDTH + 296..y * LOGICAL_WIDTH + 934],
                &old[y * LOGICAL_WIDTH + 296..y * LOGICAL_WIDTH + 934]
            );
        }
    }
}
