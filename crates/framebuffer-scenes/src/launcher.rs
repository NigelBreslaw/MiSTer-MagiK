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
mod level_trick;
mod responsive;
mod row;
const CAROUSEL_CAPACITY: usize = 8;
use crate::launcher_navigation::{BrowseDirection, BrowseFrame};
pub use level_trick::{LEVEL_TRICK_EDGE_MILLIS, LEVEL_TRICK_MILLIS, LevelChange};

/// A prepared route's selected-card placement, shared by both transition halves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CardSlot {
    pose: crate::launcher_flip::Pose,
}

impl CardSlot {
    pub fn rect(self) -> crate::navigation::NavigationTransitionRect {
        crate::navigation::NavigationTransitionRect {
            x: (self.pose.x >> 16).max(0) as u16,
            y: (self.pose.top >> 16).max(0) as u16,
            width: (self.pose.width >> 16) as u16,
            height: (self.pose.height >> 16) as u16,
        }
    }
}

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
    pub level: LauncherLevel<'a>,
}

/// The hierarchy level shown by the carousel. The root cycles its six cards
/// and describes the whole library; nested levels browse linearly, show their
/// path in the header, and describe the group the viewer is in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LauncherLevel<'a> {
    Root,
    Nested(NestedLevel<'a>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NestedLevel<'a> {
    /// Group names from the first level below the root to this group,
    /// for example `["CONSOLES", "NINTENDO"]`.
    pub path: &'a [&'a str],
    pub games: u32,
    pub children: u32,
    /// Plural noun for the cards on this level, for example `MAKERS`.
    pub children_label: &'a str,
    /// A second figure for the group, for example `(17, "SYSTEMS")`.
    pub detail: Option<(u32, &'a str)>,
    /// The collection colour carried by every card below its root card.
    pub accent: u16,
}

/// Two or more nested cards cycle. During a step, the same prepared face may
/// be drawn in the leaving-front and entering-end roles.
pub const CYCLIC_LEVEL_MIN_CARDS: usize = 2;

impl LauncherLevel<'_> {
    /// The root always cycles; nested levels cycle from two cards.
    #[must_use]
    pub const fn cyclic(&self) -> bool {
        match self {
            Self::Root => true,
            Self::Nested(level) => level.children as usize >= CYCLIC_LEVEL_MIN_CARDS,
        }
    }

    /// Nested levels slide their cards; only the end cards flip. The root
    /// keeps its flipping selection.
    #[must_use]
    pub const fn slides(&self) -> bool {
        matches!(self, Self::Nested(_))
    }
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
    pub fn slot_zero(self, nested: bool) -> CardSlot {
        let pose = if nested {
            row::slot(0)
        } else {
            continuous_geometry(0, 0, 0)
        };
        CardSlot {
            pose: responsive::Layout::for_level(self, nested)
                .map_or(pose, |layout| layout.map_pose(pose, nested)),
        }
    }

    /// Prepare card textures from high-precision source artwork.
    pub fn prepare_initial_with_rgb888_artwork(
        self,
        data: LauncherData<'_>,
        artwork: &[&[u8]],
    ) -> InitialLauncher {
        self.initial(data, Some(Artwork::Rgb888(artwork)), None)
    }

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
        #[cfg(feature = "launcher-profile")]
        let _initial_render = crate::launcher_profile::span("prepare.initial_render");
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

/// Face cache for an immutable artwork/font context. Advance `asset_generation`
/// when either input changes. Geometry and all card text/count/colour inputs
/// are compared independently; only the most recent card generation stays cached.
#[derive(Default)]
pub struct LauncherFaceCache {
    scene: Option<LauncherScene>,
    asset_generation: u64,
    slides: bool,
    artwork_kind: u8,
    keys: Vec<CardFaceKey>,
    faces: Vec<Arc<CardFaces>>,
}
#[derive(Eq, PartialEq)]
struct CardFaceKey {
    id: LauncherCardId,
    name: String,
    games: Option<u32>,
    colour: u16,
}
impl From<&LauncherCard<'_>> for CardFaceKey {
    fn from(card: &LauncherCard<'_>) -> Self {
        Self {
            id: card.id,
            name: card.name.to_owned(),
            games: card.games,
            colour: card.colour,
        }
    }
}
#[cfg(test)]
thread_local! { static FACE_BAKES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
impl LauncherScene {
    pub fn prepare_with_face_cache(
        self,
        data: LauncherData<'_>,
        cache: &mut LauncherFaceCache,
    ) -> PreparedLauncher {
        self.initial_cached(data, None, None, cache, 0).finish()
    }
    pub fn prepare_initial_with_artwork_typography_and_cache(
        self,
        data: LauncherData<'_>,
        artwork: &[&[Rgb565Pixel]],
        typography: LauncherTypography<'_>,
        cache: &mut LauncherFaceCache,
        asset_generation: u64,
    ) -> InitialLauncher {
        self.initial_cached(
            data,
            Some(Artwork::Rgb565(artwork)),
            Some(typography),
            cache,
            asset_generation,
        )
    }
    pub fn prepare_initial_with_rgb888_artwork_typography_and_cache(
        self,
        data: LauncherData<'_>,
        artwork: &[&[u8]],
        typography: LauncherTypography<'_>,
        cache: &mut LauncherFaceCache,
        asset_generation: u64,
    ) -> InitialLauncher {
        self.initial_cached(
            data,
            Some(Artwork::Rgb888(artwork)),
            Some(typography),
            cache,
            asset_generation,
        )
    }
    fn initial_cached(
        self,
        data: LauncherData<'_>,
        artwork: Option<Artwork<'_>>,
        typography: Option<LauncherTypography<'_>>,
        cache: &mut LauncherFaceCache,
        asset_generation: u64,
    ) -> InitialLauncher {
        let mut prepared = PreparedLauncher::new_cached(
            self,
            data,
            artwork,
            typography,
            Some(cache),
            asset_generation,
        );
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
    /// Pristine static chrome. Level transitions fade between two levels'
    /// chrome without re-rendering text in motion.
    chrome: Vec<Rgb565Pixel>,
    level_chrome_spans: Vec<level_trick::ChromeSpan>,
    level_chrome_alpha: Option<u32>,
    level_foreign_title: bool,
    cyclic: bool,
    fitted: Vec<Rgb565Pixel>,
    faces: Arc<Vec<Arc<CardFaces>>>,
    flip_columns: Vec<crate::launcher_flip::Scratch>,
}

struct CardFaces {
    compact: crate::launcher_flip::Face,
    detail: crate::launcher_flip::Face,
    /// The MagiK reverse of a generic card; `None` for cards with artwork.
    back: Option<crate::launcher_flip::Face>,
    /// The level's browse style, shared by all of its faces: cards slide
    /// between slots and only the end cards flip.
    slides: bool,
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
}

impl PreparedLauncherFrame {
    pub fn pixels(&self) -> &[Rgb565Pixel] {
        &self.pixels
    }
    pub fn request(&self) -> Option<LauncherFrameRequest> {
        self.request
    }
    pub fn storage_bytes(&self) -> usize {
        self.pixels.capacity() * 2
            + self
                .scratch
                .iter()
                .map(crate::launcher_flip::Scratch::storage_bytes)
                .sum::<usize>()
    }
}

#[derive(Clone)]
pub struct LauncherFramePreparer {
    faces: Arc<Vec<Arc<CardFaces>>>,
    cyclic: bool,
    trick: Option<level_trick::TrickPlan>,
}

impl LauncherFramePreparer {
    pub fn carousel_clip(&self) -> (usize, usize) {
        if self.trick.is_some() || self.faces.first().is_some_and(|face| face.slides) {
            (268, 934)
        } else {
            (296, 934)
        }
    }
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
            ..
        } = buffer;
        *rendered_request = Some(request);
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
    ) {
        assert!(destination.len() >= 960 * 540);
        buffer.request = Some(request);
        self.render_tile_pixels(request, &mut buffer.scratch, destination, clip);
    }

    fn render_tile_pixels(
        &self,
        request: LauncherFrameRequest,
        scratch: &mut [crate::launcher_flip::Scratch],
        pixels: &mut [Rgb565Pixel],
        clip: (usize, usize),
    ) {
        assert!(clip.0 >= self.carousel_clip().0 && clip.0 <= clip.1 && clip.1 <= 934);
        {
            #[cfg(feature = "launcher-profile")]
            let _clear = crate::launcher_profile::span("flip.clear");
            for y in 120..495 {
                pixels[y * 960 + clip.0..y * 960 + clip.1].fill(Rgb565Pixel(0));
            }
        }
        if !self.faces.is_empty() {
            let plan = self.trick.map_or_else(
                || build_carousel_plan(&self.faces, request.frame, self.cyclic),
                |plan| plan.with_faces(&self.faces),
            );
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
        let mut buffer = self.new_direct_tile_buffer();
        buffer.pixels = vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        buffer
    }

    pub(crate) fn new_direct_tile_buffer(&self) -> PreparedLauncherFrame {
        PreparedLauncherFrame {
            request: None,
            scratch: (0..CAROUSEL_CAPACITY)
                .map(|_| crate::launcher_flip::Scratch::strip())
                .collect(),
            pixels: Vec::new(),
        }
    }
}

fn bake_face(
    card: &PreparedCard<'_>,
    width: usize,
    selected: bool,
    typography: Option<LauncherTypography<'_>>,
    cache: &mut artwork::BodyCache,
) -> crate::launcher_flip::Face {
    artwork::face_cached(card, width, selected, typography, cache)
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
    pub fn slot_zero(&self) -> CardSlot {
        self.scene
            .slot_zero(self.faces.first().is_some_and(|face| face.slides))
    }

    fn resting_pose(&self, relative: isize) -> crate::launcher_flip::Pose {
        let nested = self.faces.first().is_some_and(|face| face.slides);
        let mut pose = if nested {
            row::slot(relative as usize)
        } else {
            continuous_geometry(relative, relative, 0)
        };
        if nested && self.scene.crt {
            pose.angle = 0;
        }
        self.responsive
            .map_or(pose, |layout| layout.map_pose(pose, nested))
    }

    pub fn carousel_clip(&self) -> (usize, usize) {
        self.frame_preparer().carousel_clip()
    }
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
        self.chrome.copy_from_slice(&self.logical);
        self.rebuild_level_chrome_spans();
        self.fit_output();
    }

    pub fn frame_preparer(&self) -> LauncherFramePreparer {
        LauncherFramePreparer {
            faces: self.faces.clone(),
            cyclic: self.cyclic,
            trick: None,
        }
    }
    /// Owned raster-buffer capacity, excluding strings and small metadata.
    /// This is not process RSS; it makes the quality/cache tradeoff measurable.
    pub fn cached_raster_bytes(&self) -> usize {
        (self.logical.capacity() + self.chrome.capacity() + self.fitted.capacity()) * 2
            + self
                .faces
                .iter()
                .map(|f| {
                    f.compact.storage_bytes()
                        + f.detail.storage_bytes()
                        + f.back
                            .as_ref()
                            .map_or(0, crate::launcher_flip::Face::storage_bytes)
                })
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
        Self::new_cached(scene, data, artwork, typography, None, 0)
    }

    fn new_cached(
        scene: LauncherScene,
        data: LauncherData<'_>,
        artwork: Option<Artwork<'_>>,
        typography: Option<LauncherTypography<'_>>,
        cache: Option<&mut LauncherFaceCache>,
        asset_generation: u64,
    ) -> Self {
        #[cfg(feature = "launcher-profile")]
        let _preparation = crate::launcher_profile::span("prepare.launcher_constructor");
        let keys: Vec<_> = data.cards.iter().map(CardFaceKey::from).collect();
        let artwork_kind = match artwork {
            None => 0,
            Some(Artwork::Rgb565(_)) => 1,
            Some(Artwork::Rgb888(_)) => 2,
        };
        let reusable = cache.as_ref().filter(|cache| {
            cache.scene == Some(scene)
                && cache.asset_generation == asset_generation
                && cache.slides == data.level.slides()
                && cache.artwork_kind == artwork_kind
        });
        let responsive = responsive::Layout::for_level(scene, data.level.slides());
        let fonts = responsive.map(|layout| layout.fonts(typography));
        let pixel_count = if responsive.is_some() {
            scene.width * scene.height
        } else {
            LOGICAL_WIDTH * LOGICAL_HEIGHT
        };
        #[cfg(feature = "launcher-profile")]
        let chrome_span = crate::launcher_profile::span("prepare.chrome");
        let mut chrome = vec![Rgb565Pixel(BACKGROUND); pixel_count];
        if let Some((layout, fonts)) = responsive.as_ref().zip(fonts.as_ref()) {
            layout.chrome(&mut chrome, data, fonts);
        } else {
            render_logical(&mut chrome, data, typography);
        }
        #[cfg(feature = "launcher-profile")]
        drop(chrome_span);
        let mut bodies = artwork::BodyCache::default();
        let faces: Vec<_> = data
            .cards
            .iter()
            .enumerate()
            .map(|(index, card)| {
                #[cfg(feature = "launcher-profile")]
                let _card = crate::launcher_profile::span(match index {
                    0 => "prepare.card0",
                    1 => "prepare.card1",
                    2 => "prepare.card2",
                    3 => "prepare.card3",
                    4 => "prepare.card4",
                    5 => "prepare.card5",
                    _ => "prepare.card_other",
                });
                if let Some(cache) = reusable
                    && cache.keys.get(index) == Some(&keys[index])
                {
                    return Arc::clone(&cache.faces[index]);
                }
                let card = PreparedCard {
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
                };
                #[cfg(test)]
                FACE_BAKES.set(FACE_BAKES.get() + 2);
                let mut faces = if card.rgb888.is_some() && responsive.is_none() {
                    #[cfg(feature = "launcher-profile")]
                    let _faces = crate::launcher_profile::span("prepare.rgb888_faces");
                    let [compact, detail] = artwork::faces_rgb888(&card, typography);
                    CardFaces {
                        compact,
                        detail,
                        back: None,
                        slides: data.level.slides(),
                    }
                } else {
                    #[cfg(feature = "launcher-profile")]
                    let _faces = crate::launcher_profile::span("prepare.initial_faces");
                    if let Some((layout, fonts)) = responsive.as_ref().zip(fonts.as_ref()) {
                        layout.faces(&card, fonts, &mut bodies, data.level.slides())
                    } else {
                        CardFaces {
                            compact: bake_face(&card, 180, false, typography, &mut bodies),
                            detail: bake_face(&card, 180, true, typography, &mut bodies),
                            back: bodies.back_face(&card),
                            slides: data.level.slides(),
                        }
                    }
                };
                let dithered = responsive.is_none();
                faces.compact.dithered = dithered;
                faces.detail.dithered = dithered;
                if let Some(back) = &mut faces.back {
                    back.dithered = dithered;
                }
                Arc::new(faces)
            })
            .collect();
        if let Some(cache) = cache {
            cache.scene = Some(scene);
            cache.asset_generation = asset_generation;
            cache.slides = data.level.slides();
            cache.artwork_kind = artwork_kind;
            cache.keys = keys;
            cache.faces = faces.clone();
        }
        #[cfg(feature = "launcher-profile")]
        let _buffers = crate::launcher_profile::span("prepare.retained_buffers");
        let mut prepared = Self {
            scene,
            responsive,
            chrome: chrome.clone(),
            level_chrome_spans: Vec::new(),
            level_chrome_alpha: None,
            level_foreign_title: false,
            cyclic: data.level.cyclic(),
            logical: chrome,
            fitted: if responsive.is_some()
                || (scene.width == LOGICAL_WIDTH && scene.height == LOGICAL_HEIGHT)
            {
                Vec::new()
            } else {
                vec![Rgb565Pixel(BACKGROUND); scene.width * scene.height]
            },
            faces: Arc::new(faces),
            flip_columns: (0..if data.level.slides() {
                CAROUSEL_CAPACITY
            } else {
                6
            })
                .map(|_| {
                    if let Some(layout) = responsive {
                        crate::launcher_flip::Scratch::sized(
                            crate::launcher_flip::STRIP_WIDTH,
                            scene.width,
                            layout.card_h,
                        )
                    } else {
                        crate::launcher_flip::Scratch::strip()
                    }
                })
                .collect(),
        };
        prepared.rebuild_level_chrome_spans();
        prepared
    }

    pub fn merge_retained_helper(
        &mut self,
        renderer: &mut crate::launcher_parallel::ParallelLauncherRenderer,
    ) {
        renderer.merge_retained_helper(&mut self.logical);
    }

    pub fn render_parallel_frame(
        &mut self,
        renderer: &mut crate::launcher_parallel::ParallelLauncherRenderer,
        request: LauncherFrameRequest,
    ) -> Result<crate::launcher_parallel::ParallelFrameTiming, String> {
        if self.scene != LauncherScene::new(960, 540) {
            return Err("parallel cards require native geometry".into());
        }
        renderer.render(&self.frame_preparer(), request, &mut self.logical)
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
                self.cyclic,
                &mut self.flip_columns,
            );
            return;
        }
        #[cfg(feature = "launcher-profile")]
        let clear_profile = crate::launcher_profile::span("scene.clear");
        // All animation, including projected edges and reflections, is clipped
        // to this region. Keep static chrome resident between frames.
        for rect in self.logical_damage() {
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
        let plan = build_carousel_plan(&self.faces, frame, self.cyclic);
        for left in (self.carousel_clip().0..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
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

    fn logical_damage(&self) -> [crate::Rgb565Rect; 1] {
        [crate::Rgb565Rect {
            x0: self.carousel_clip().0,
            y0: 120,
            x1: 934,
            y1: 495,
        }]
    }

    /// Conservative union of every previous/current card pose and reflection.
    /// At other output sizes, fitting still invalidates the complete surface.
    pub fn damage(&self) -> [crate::Rgb565Rect; 1] {
        if self.scene.width == LOGICAL_WIDTH && self.scene.height == LOGICAL_HEIGHT {
            self.logical_damage()
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
    if let LauncherLevel::Nested(level) = data.level {
        draw_breadcrumb(pixels, typography, level.path);
    } else {
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
    }
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
    let section = match data.level {
        LauncherLevel::Root => {
            draw_library_sidebar(pixels, data, typography);
            "COLLECTIONS"
        }
        LauncherLevel::Nested(level) => {
            draw_group_sidebar(pixels, level, typography);
            level.children_label
        }
    };
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        296,
        101,
        section,
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

fn draw_library_sidebar(
    pixels: &mut [Rgb565Pixel],
    data: LauncherData<'_>,
    typography: Option<LauncherTypography<'_>>,
) {
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
}

/// Nested levels describe the group the viewer is in, in the same places the
/// root describes the whole library.
fn draw_group_sidebar(
    pixels: &mut [Rgb565Pixel],
    level: NestedLevel<'_>,
    typography: Option<LauncherTypography<'_>>,
) {
    let name = level.path.last().copied().unwrap_or("");
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        29,
        101,
        name,
        MUTED,
        1,
    );
    draw_role_number(pixels, typography, 28, 142, level.games, CREAM, 5);
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
    draw_role_number(pixels, typography, 30, 265, level.children, CREAM, 3);
    if let Some((value, _)) = level.detail {
        draw_role_number(pixels, typography, 150, 265, value, CREAM, 3);
    }
    draw_role_text(
        pixels,
        typography,
        TextRole::Metadata,
        30,
        310,
        level.children_label,
        MUTED,
        1,
    );
    if let Some((_, label)) = level.detail {
        draw_role_text(
            pixels,
            typography,
            TextRole::Metadata,
            150,
            310,
            label,
            MUTED,
            1,
        );
    }
    draw_line(pixels, 28, 340, 240, 340, RULE);
    draw_rect(pixels, 29, 436, 210, 7, level.accent);
}

/// `CONSOLES / NINTENDO`: ancestors muted, the current group in cream.
fn draw_breadcrumb(
    pixels: &mut [Rgb565Pixel],
    typography: Option<LauncherTypography<'_>>,
    path: &[&str],
) {
    let mut x = 26;
    for (index, name) in path.iter().enumerate() {
        let last = index + 1 == path.len();
        let colour = if last { CREAM } else { MUTED };
        draw_role_text(
            pixels,
            typography,
            TextRole::Heading,
            x,
            20,
            name,
            colour,
            3,
        );
        x += role_text_width(typography, TextRole::Heading, name, 3);
        if !last {
            let separator = " / ";
            draw_role_text(
                pixels,
                typography,
                TextRole::Heading,
                x,
                20,
                separator,
                MUTED,
                3,
            );
            x += role_text_width(typography, TextRole::Heading, separator, 3);
        }
    }
}

fn role_text_width(
    typography: Option<LauncherTypography<'_>>,
    role: TextRole,
    text: &str,
    legacy_scale: usize,
) -> usize {
    typography.map_or(text.chars().count() * 6 * legacy_scale, |fonts| {
        fonts.font_for(role, text).measure(text)
    })
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
        brightness: 256,
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

/// How far an end card turns as it leaves or enters a sliding level: 150
/// degrees, so a generic card shows its MagiK back before it is gone.
const SLIDE_FLIP: i64 = GEOMETRY_ONE * 5 / 6;

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
    row: bool,
    items: [Option<CarouselItem<'a>>; CAROUSEL_CAPACITY],
}

fn build_carousel_plan<'a>(
    faces: &'a [Arc<CardFaces>],
    mut motion: BrowseFrame,
    cyclic: bool,
) -> CarouselPlan<'a> {
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
    if faces.first().is_some_and(|face| face.slides) {
        return row::build(faces, motion);
    }
    let selected = motion.selected % faces.len();
    let progress = if settled {
        0
    } else {
        smooth_progress(motion.progress_millis, motion.duration_millis)
    };
    let right = motion.direction == Some(BrowseDirection::Right);
    let slide = faces.first().is_some_and(|face| face.slides);
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
    let mut items = [None; CAROUSEL_CAPACITY];
    for (slot, relative) in relatives.iter().enumerate() {
        let position = selected as isize + *relative;
        if !cyclic && (position < 0 || position >= faces.len() as isize) {
            continue;
        }
        let index = position.rem_euclid(faces.len() as isize) as usize;
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
        // Sliding levels never flip the selection. The card leaving through
        // one end turns away, and the card entering at the other end turns
        // in, showing its MagiK back part of the way.
        let in_motion = !settled && progress > 0 && progress < GEOMETRY_ONE;
        let leaving = slide && in_motion && relative.abs() == 2 && destination.abs() == 3;
        let entering = slide && in_motion && relative.abs() == 3 && destination.abs() == 2;
        let flipping_card = !slide
            && (incoming || *relative == 0)
            && progress > 0
            && progress < GEOMETRY_ONE
            && motion.phase == crate::launcher_navigation::BrowsePhase::Flipping;
        let (face, blend) = if leaving || entering {
            let turn = if leaving {
                progress
            } else {
                progress - GEOMETRY_ONE
            };
            let extra = flip_spin(right) * SLIDE_FLIP * turn / GEOMETRY_ONE;
            pose.angle += extra;
            let face = match &faces[index].back {
                Some(back) if extra.abs() > GEOMETRY_ONE / 2 => back,
                _ => &faces[index].compact,
            };
            (face, None)
        } else if flipping_card {
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
    CarouselPlan { items, row: false }
}

fn draw_carousel_plan(
    pixels: &mut [Rgb565Pixel],
    pitch: usize,
    origin: (usize, usize),
    plan: &CarouselPlan<'_>,
    scratch: &mut [crate::launcher_flip::Scratch],
    clip: (usize, usize),
) {
    draw_carousel_plan_prepared::<true>(pixels, pitch, origin, plan, scratch, clip);
}

fn draw_carousel_plan_prepared<const CULL_SOURCE: bool>(
    pixels: &mut [Rgb565Pixel],
    pitch: usize,
    origin: (usize, usize),
    plan: &CarouselPlan<'_>,
    scratch: &mut [crate::launcher_flip::Scratch],
    clip: (usize, usize),
) {
    let mut covered = crate::launcher_flip::BodyOcclusion::new(clip);
    let mut occlusion = [covered; CAROUSEL_CAPACITY];
    if CULL_SOURCE {
        // Prepare front to back so only proven-opaque foreground spans
        // can remove source filtering from the cards behind them.
        for (slot, item) in plan.items.iter().enumerate().rev() {
            occlusion[slot] = covered;
            let Some(item) = item else { continue };
            let mut pose = item.pose;
            pose.clip = (pose.clip.0.max(clip.0), pose.clip.1.min(clip.1));
            if pose.clip.0 >= pose.clip.1 {
                continue;
            }
            pose.body_clip.0 = pose.body_clip.0.max(clip.0).min(clip.1);
            pose.body_clip.1 = pose.body_clip.1.min(clip.1).max(clip.0);
            crate::launcher_flip::prepare_target(
                pixels,
                pitch,
                origin,
                item.face,
                pose,
                &mut scratch[slot],
                item.blend,
                &covered,
            );
            crate::launcher_flip::add_opaque_coverage(
                item.face,
                pose,
                &scratch[slot],
                &mut covered,
            );
        }
    }
    draw_carousel_reflections(pixels, pitch, origin, plan, scratch, clip);
    if !CULL_SOURCE {
        // Reference path prepares every column before applying body occlusion.
        for (slot, item) in plan.items.iter().enumerate().rev() {
            occlusion[slot] = covered;
            let Some(item) = item else { continue };
            let mut pose = item.pose;
            pose.clip = (pose.clip.0.max(clip.0), pose.clip.1.min(clip.1));
            if pose.clip.0 >= pose.clip.1 {
                continue;
            }
            pose.body_clip.0 = pose.body_clip.0.max(clip.0).min(clip.1);
            pose.body_clip.1 = pose.body_clip.1.min(clip.1).max(clip.0);
            crate::launcher_flip::add_opaque_coverage(
                item.face,
                pose,
                &scratch[slot],
                &mut covered,
            );
        }
    }
    for (slot, item) in plan.items.iter().enumerate() {
        let Some(item) = item else { continue };
        let mut pose = item.pose;
        pose.clip = (pose.clip.0.max(clip.0), pose.clip.1.min(clip.1));
        if pose.clip.0 >= pose.clip.1 {
            continue;
        }
        pose.body_clip.0 = pose.body_clip.0.max(clip.0).min(clip.1);
        pose.body_clip.1 = pose.body_clip.1.min(clip.1).max(clip.0);
        crate::launcher_flip::draw_occluded_target(
            pixels,
            pitch,
            origin,
            item.face,
            pose,
            &mut scratch[slot],
            item.blend,
            &occlusion[slot],
        );
    }
}

fn draw_carousel_reflections(
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
        pose.clip = (pose.clip.0.max(clip.0), pose.clip.1.min(clip.1));
        if pose.clip.0 >= pose.clip.1 {
            continue;
        }
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
            level: LauncherLevel::Root,
        }
    }

    /// Root projection, lighting, face swaps and reflections match the independently
    /// rendered origin/main baseline at 8793765ae. Artwork is the high-precision source shipped by main.
    #[test]
    fn root_artwork_motion_keeps_approved_raster_contract() {
        let asset_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/mister/assets/ui/launcher-cards");
        let artwork: Vec<Vec<u8>> = [
            "01_arcade",
            "02_consoles",
            "03_computers",
            "04_handhelds",
            "05_favourites",
        ]
        .iter()
        .map(|name| std::fs::read(asset_root.join(format!("{name}.rgb888"))).unwrap())
        .collect();
        let faces: Vec<_> = artwork.iter().map(Vec::as_slice).collect();
        let scenes = [
            LauncherScene::new(960, 540),
            LauncherScene::new(540, 960),
            LauncherScene::crt(640, 240),
            LauncherScene::crt(240, 640),
        ];
        let mut actual = Vec::new();
        for scene in scenes {
            let mut prepared =
                PreparedLauncher::new(scene, data(), Some(Artwork::Rgb888(&faces)), None);
            for direction in [BrowseDirection::Right, BrowseDirection::Left] {
                let selected = if direction == BrowseDirection::Right {
                    4
                } else {
                    0
                };
                let target = if direction == BrowseDirection::Right {
                    0
                } else {
                    4
                };
                for progress_millis in [0, 16384, 32768, 49152, 65536] {
                    prepared.render_frame(BrowseFrame {
                        selected,
                        target,
                        direction: Some(direction),
                        phase: crate::launcher_navigation::BrowsePhase::Flipping,
                        progress_millis,
                        duration_millis: crate::launcher_navigation::SPRING_POSITION_UNITS,
                    });
                    let hash = prepared
                        .pixels()
                        .iter()
                        .flat_map(|p| p.0.to_le_bytes())
                        .fold(0xcbf29ce484222325_u64, |h, b| {
                            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                        });
                    actual.push(hash);
                }
            }
        }
        assert_eq!(
            actual,
            vec![
                0xc394f8d44e82ff4d,
                0x5f62be6910db80df,
                0xa3fa917c6962bb37,
                0x4b48f8013bfe834c,
                0x60daa1b771e35d5f,
                0x60daa1b771e35d5f,
                0x732f9fcb676fe381,
                0x4be592b8f2e236d6,
                0xc483a8a561c8311e,
                0xc394f8d44e82ff4d,
                0xf8ac5e551d39f36b,
                0x5d33bcef2c74e893,
                0xf3ea3c00d4d2fe0d,
                0x9193dac43efe10b3,
                0x86e5f9ae0ec9adfc,
                0x86e5f9ae0ec9adfc,
                0x442179c3c212a1f4,
                0x82762e587e373153,
                0x2f93fe430446b37d,
                0xf8ac5e551d39f36b,
                0x7aad31d1ab0c8236,
                0x28d9c2b5e17eb9d5,
                0x42ad22aec66929a3,
                0xc1391c3dd578d056,
                0xb8fd1aacb7e7c1f1,
                0xb8fd1aacb7e7c1f1,
                0x3095685d339eb4c1,
                0x9dc46ab3e57739a9,
                0x5eace0b7da247b09,
                0x7aad31d1ab0c8236,
                0x104012a1161a17ed,
                0x64a2391606ca0e25,
                0xc2e394552026b0e4,
                0x32393fdbabb310d3,
                0xc4c238e3e9ad794c,
                0xc4c238e3e9ad794c,
                0x8c04e43ed9a03ddc,
                0x3c64861760b62cbc,
                0x76f9d2f9120a3679,
                0x104012a1161a17ed,
            ],
            "Root baseline: {actual:x?}"
        );
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
    fn culled_source_preparation_matches_full_columns_through_motion_and_reversal() {
        let prepared = LauncherScene::new(960, 540).prepare(data());
        let mut culled_scratch: Vec<_> = (0..6)
            .map(|_| crate::launcher_flip::Scratch::strip())
            .collect();
        let mut full_scratch: Vec<_> = (0..6)
            .map(|_| crate::launcher_flip::Scratch::strip())
            .collect();
        for direction in [BrowseDirection::Right, BrowseDirection::Left] {
            for (selected, target) in [(0, 1), (4, 0)] {
                for progress in [0, 1, 30, 89, 91, 140, 179, 180, 140, 91, 30] {
                    let frame = BrowseFrame {
                        selected,
                        target,
                        phase: crate::launcher_navigation::BrowsePhase::Flipping,
                        direction: Some(direction),
                        progress_millis: progress,
                        duration_millis: 180,
                    };
                    let plan = build_carousel_plan(&prepared.faces, frame, true);
                    // Keep the scratch alive between poses: newly uncovered
                    // source rows must be prepared after a reversal or wrap.
                    for left in (296..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
                        let right = (left + crate::launcher_flip::STRIP_WIDTH).min(934);
                        let mut culled = vec![Rgb565Pixel(BACKGROUND); (right - left) * 375];
                        let mut full = culled.clone();
                        draw_carousel_plan_prepared::<true>(
                            &mut culled,
                            right - left,
                            (left, 120),
                            &plan,
                            &mut culled_scratch,
                            (left, right),
                        );
                        draw_carousel_plan_prepared::<false>(
                            &mut full,
                            right - left,
                            (left, 120),
                            &plan,
                            &mut full_scratch,
                            (left, right),
                        );
                        assert!(
                            culled == full,
                            "pixel mismatch: {direction:?} {selected}->{target} progress={progress} strip={left}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn one_card_change_reuses_unchanged_faces_and_generic_backs() {
        for nested in [false, true] {
            let scene = LauncherScene::new(960, 540);
            let mut cache = LauncherFaceCache::default();
            let mut cards = [data().cards[0]; 6];
            if nested {
                for card in &mut cards {
                    card.id = LauncherCardId::Consoles;
                    card.name = "NINTENDO";
                }
            }
            let level = if nested {
                LauncherLevel::Nested(NestedLevel {
                    path: &["CONSOLES"],
                    games: 60,
                    children: 6,
                    children_label: "MAKERS",
                    detail: Some((9, "SYSTEMS")),
                    accent: 0x2a7f,
                })
            } else {
                LauncherLevel::Root
            };
            let mut input = data();
            input.cards = &cards;
            input.level = level;
            let first = scene.prepare_with_face_cache(input, &mut cache);
            cards[0].games = Some(999);
            let mut input = data();
            input.cards = &cards;
            input.level = level;
            FACE_BAKES.set(0);
            let mut updated = scene.prepare_with_face_cache(input, &mut cache);
            let bakes = FACE_BAKES.get();
            println!("changed_card_front_face_bakes={bakes} nested={nested}");
            assert_eq!(bakes, 2);
            assert!(!Arc::ptr_eq(&first.faces[0], &updated.faces[0]));
            for index in 1..6 {
                assert!(Arc::ptr_eq(&first.faces[index], &updated.faces[index]));
                assert_eq!(updated.faces[index].back.is_some(), nested);
            }
            let mut reference = scene.prepare(input);
            for phase in [0, 1, 90, 179, 180] {
                let frame = BrowseFrame {
                    selected: 0,
                    target: 1,
                    phase: crate::launcher_navigation::BrowsePhase::Flipping,
                    direction: Some(crate::launcher_navigation::BrowseDirection::Right),
                    progress_millis: phase,
                    duration_millis: 180,
                };
                updated.render_frame(frame);
                reference.render_frame(frame);
                assert!(
                    updated.pixels() == reference.pixels(),
                    "motion differs at {phase}, nested={nested}"
                );
            }
            let slot = scene.slot_zero(nested);
            for elapsed in [0, 200, 400, 600, 920] {
                updated.render_level_gather_to(0, LevelChange::Descend, elapsed, slot);
                reference.render_level_gather_to(0, LevelChange::Descend, elapsed, slot);
                assert!(
                    updated.pixels() == reference.pixels(),
                    "gather differs at {elapsed}"
                );
                updated.render_level_deal_from(0, LevelChange::Ascend, elapsed, slot);
                reference.render_level_deal_from(0, LevelChange::Ascend, elapsed, slot);
                assert!(
                    updated.pixels() == reference.pixels(),
                    "deal differs at {elapsed}"
                );
            }
        }
    }

    #[test]
    fn face_cache_invalidates_browse_style_geometry_and_asset_context() {
        let scene = LauncherScene::new(960, 540);
        let mut cache = LauncherFaceCache::default();
        let input = data();
        let _first = scene.prepare_with_face_cache(input, &mut cache);
        let mut nested = input;
        nested.level = LauncherLevel::Nested(NestedLevel {
            path: &["CONSOLES"],
            games: 60,
            children: 6,
            children_label: "MAKERS",
            detail: None,
            accent: 0x2a7f,
        });
        FACE_BAKES.set(0);
        let updated = scene.prepare_with_face_cache(nested, &mut cache);
        assert_eq!(FACE_BAKES.get(), input.cards.len() * 2);
        assert!(updated.pixels() == scene.prepare(nested).pixels());
        for scene in [
            LauncherScene::new(540, 960),
            LauncherScene::crt(640, 480),
            LauncherScene::crt(480, 640),
        ] {
            FACE_BAKES.set(0);
            let updated = scene.prepare_with_face_cache(nested, &mut cache);
            assert_eq!(FACE_BAKES.get(), input.cards.len() * 2);
            assert!(updated.pixels() == scene.prepare(nested).pixels());
        }
        let scene = LauncherScene::crt(480, 640);
        FACE_BAKES.set(0);
        let _updated = scene.initial_cached(nested, None, None, &mut cache, 1);
        assert_eq!(FACE_BAKES.get(), input.cards.len() * 2);
        FACE_BAKES.set(0);
        let _same = scene.initial_cached(nested, None, None, &mut cache, 1);
        assert_eq!(FACE_BAKES.get(), 0);
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
            for rect in reference.logical_damage() {
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
                let plan = build_carousel_plan(&prepared.faces, motion, true);
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
                true,
            );
            assert_eq!(settled.items[4].expect("settled top card").pose.angle, 0);
        }
    }

    #[test]
    fn motion_renders_reflections_for_every_visible_card_at_both_edges() {
        let prepared = LauncherScene::new(960, 540).prepare(data());
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        for (direction, target) in [(BrowseDirection::Right, 1), (BrowseDirection::Left, 4)] {
            for progress in [1, units / 2, units - 1] {
                let moving = build_carousel_plan(
                    &prepared.faces,
                    BrowseFrame {
                        selected: 0,
                        target,
                        phase: crate::launcher_navigation::BrowsePhase::Flipping,
                        direction: Some(direction),
                        progress_millis: progress,
                        duration_millis: units,
                    },
                    true,
                );
                let mut reflections = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
                let mut reflection_scratch: Vec<_> = (0..CAROUSEL_CAPACITY)
                    .map(|_| crate::launcher_flip::Scratch::strip())
                    .collect();
                for left in (296..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
                    draw_carousel_reflections(
                        &mut reflections,
                        LOGICAL_WIDTH,
                        (0, 0),
                        &moving,
                        &mut reflection_scratch,
                        (left, (left + crate::launcher_flip::STRIP_WIDTH).min(934)),
                    );
                }

                for (slot, item) in moving.items.iter().enumerate() {
                    if item.is_none() {
                        continue;
                    }
                    let mut without = CarouselPlan {
                        row: moving.row,
                        items: moving.items,
                    };
                    without.items[slot] = None;
                    let mut without_pixels = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
                    let mut without_scratch: Vec<_> = (0..CAROUSEL_CAPACITY)
                        .map(|_| crate::launcher_flip::Scratch::strip())
                        .collect();
                    for left in (296..934).step_by(crate::launcher_flip::STRIP_WIDTH) {
                        draw_carousel_reflections(
                            &mut without_pixels,
                            LOGICAL_WIDTH,
                            (0, 0),
                            &without,
                            &mut without_scratch,
                            (left, (left + crate::launcher_flip::STRIP_WIDTH).min(934)),
                        );
                    }
                    assert_ne!(
                        reflections, without_pixels,
                        "moving card in slot {slot} contributed no visible reflection at {progress} going {direction:?}"
                    );
                }
            }
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
