// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Static text-and-colour launcher scene used by the Mini-MagiK visual probe.
//!
//! The scene deliberately has no Slint or runtime dependency. It renders a
//! packed RGB565 frame with native portrait/CRT layouts and the original
//! 960x540 landscape composition.

use crate::Rgb565Pixel;
use crate::bitmap_text::BitmapFont;
use crate::launcher_flip::Dither;
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

/// Collection styling shared by runtime navigation and host artwork generation.
#[derive(Clone, Copy)]
pub struct LauncherCardStyle {
    pub id: LauncherCardId,
    pub colour: u16,
}
impl LauncherCardStyle {
    pub const fn root(id: LauncherCardId) -> Self {
        let colour = match id {
            LauncherCardId::Arcade => 0xe1a5,
            LauncherCardId::Consoles => 0x2a7f,
            LauncherCardId::Computers => 0xedc6,
            LauncherCardId::Handhelds => 0x2df2,
            LauncherCardId::Favourites => 0xe12f,
            LauncherCardId::Settings => 0x8b7f,
        };
        Self { id, colour }
    }
    pub fn section(section: &str) -> Self {
        Self::root(match section {
            "computers" => LauncherCardId::Computers,
            "handhelds" => LauncherCardId::Handhelds,
            _ => LauncherCardId::Consoles,
        })
    }
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

    /// Prepare card faces from 360x504 RGB888 source artwork. Every output shares
    /// one 180x252 face per card, which the projection scales to each output's
    /// card size; scanout remains RGB565.
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

/// One lazily supplied source. Retryable fallbacks are reused until an explicit
/// retry; their pixels must stay identical within an artwork generation. Source
/// pixels are dropped after this card bakes.
pub mod prepared_artwork;

pub struct LauncherArtwork {
    pub prepared: Option<prepared_artwork::PreparedArtwork>,
    pub pixels: std::borrow::Cow<'static, [u8]>,
    pub retry: bool,
    /// The validated image contains its own wordmark; omit the duplicate title.
    pub contains_name: bool,
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
    retry_artwork: bool,
}
impl LauncherFaceCache {
    /// Only an explicit background retry may revisit failed artwork.
    pub fn retry_failed_artwork(&mut self) {
        self.retry_artwork = true;
    }
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
    /// Only calls `load` for faces that cannot be reused. The callback and its
    /// pixels live exclusively during preparation, never during frame rendering.
    pub fn prepare_initial_with_rgb888_loader_and_cache(
        self,
        data: LauncherData<'_>,
        load: &mut dyn FnMut(usize) -> LauncherArtwork,
        typography: Option<LauncherTypography<'_>>,
        cache: &mut LauncherFaceCache,
        asset_generation: u64,
    ) -> InitialLauncher {
        let mut prepared = PreparedLauncher::new_cached_with_loader(
            self,
            data,
            None,
            typography,
            Some(cache),
            asset_generation,
            Some(load),
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
    chrome_state: ChromeState,
    level_chrome_spans: Vec<level_trick::ChromeSpan>,
    level_chrome_alpha: Option<u32>,
    level_foreign_title: bool,
    cyclic: bool,
    fitted: Vec<Rgb565Pixel>,
    retry_artwork: bool,
    faces: Arc<Vec<Arc<CardFaces>>>,
    flip_columns: Vec<crate::launcher_flip::Scratch>,
}

struct ChromeState {
    cards: Vec<CardFaceKey>,
    clock: String,
    selected: usize,
    totals: (u32, u32, u32),
    nested: Option<OwnedNested>,
}
struct OwnedNested {
    path: Vec<String>,
    games: u32,
    children: u32,
    label: String,
    detail: Option<(u32, String)>,
    accent: u16,
}
impl ChromeState {
    fn new(data: LauncherData<'_>) -> Self {
        Self {
            cards: data.cards.iter().map(CardFaceKey::from).collect(),
            clock: data.clock.to_owned(),
            selected: data.selected,
            totals: (data.library_games, data.collections, data.favourites),
            nested: match data.level {
                LauncherLevel::Root => None,
                LauncherLevel::Nested(n) => Some(OwnedNested {
                    path: n.path.iter().map(|s| (*s).to_owned()).collect(),
                    games: n.games,
                    children: n.children,
                    label: n.children_label.to_owned(),
                    detail: n.detail.map(|(v, s)| (v, s.to_owned())),
                    accent: n.accent,
                }),
            },
        }
    }
    fn matches(&self, data: LauncherData<'_>) -> bool {
        self.clock == data.clock
            && self.selected == data.selected
            && self.totals == (data.library_games, data.collections, data.favourites)
            && self.cards.len() == data.cards.len()
            && self.cards.iter().zip(data.cards).all(|(a, b)| {
                a.id == b.id && a.name == b.name && a.games == b.games && a.colour == b.colour
            })
            && match (&self.nested, data.level) {
                (None, LauncherLevel::Root) => true,
                (Some(a), LauncherLevel::Nested(b)) => {
                    a.path.iter().map(String::as_str).eq(b.path.iter().copied())
                        && a.games == b.games
                        && a.children == b.children
                        && a.label == b.children_label
                        && a.detail.as_ref().map(|(v, s)| (*v, s.as_str())) == b.detail
                        && a.accent == b.accent
                }
                _ => false,
            }
    }
}

struct CardFaces {
    source_retry: bool,
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
    pub(super) fn spare_pixels() -> Self {
        Self {
            request: None,
            scratch: Vec::new(),
            pixels: vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT],
        }
    }

    pub(super) fn swap_scratch(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.scratch, &mut other.scratch);
    }

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
    pub(super) fn same_source(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.faces, &other.faces)
            && self.cyclic == other.cyclic
            && self.trick == other.trick
    }

    pub fn carousel_clip(&self) -> (usize, usize) {
        CardRow::canvas(self.trick.is_some() || self.faces.first().is_some_and(|face| face.slides))
            .clip
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
        assert!(clip.0 >= self.carousel_clip().0 && clip.0 <= clip.1 && clip.1 <= CardRow::RIGHT);
        {
            #[cfg(feature = "launcher-profile")]
            let _clear = crate::launcher_profile::span("flip.clear");
            clear_card_rows(
                pixels,
                LOGICAL_WIDTH,
                CardRow::canvas(false).with_clip(clip),
            );
        }
        if !self.faces.is_empty() {
            let plan = self.trick.map_or_else(
                || build_carousel_plan(&self.faces, request.frame, self.cyclic),
                |plan| plan.with_faces(&self.faces),
            );
            draw_card_strips(
                pixels,
                LOGICAL_WIDTH,
                CardRow::canvas(false).with_clip(clip),
                &plan,
                scratch,
            );
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

#[allow(clippy::too_many_arguments)]
fn bake_face(
    card: &PreparedCard<'_>,
    width: usize,
    height: usize,
    selected: bool,
    typography: Option<LauncherTypography<'_>>,
    narrow_title: Option<&BitmapFont>,
    cache: &mut artwork::BodyCache,
) -> crate::launcher_flip::Face {
    artwork::face_cached(
        card,
        width,
        height,
        selected,
        typography,
        narrow_title,
        cache,
    )
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
    /// Data identity only; callers retain an immutable typography context.
    pub fn chrome_matches(&self, data: LauncherData<'_>) -> bool {
        self.chrome_state.matches(data)
    }
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
        self.chrome_state = ChromeState::new(data);
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
    pub fn shares_faces_with(&self, other: &Self) -> bool {
        self.faces.len() == other.faces.len()
            && self
                .faces
                .iter()
                .zip(other.faces.iter())
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
    pub fn needs_artwork_retry(&self) -> bool {
        self.retry_artwork
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
        Self::new_cached_with_loader(
            scene,
            data,
            artwork,
            typography,
            cache,
            asset_generation,
            None,
        )
    }

    fn new_cached_with_loader(
        scene: LauncherScene,
        data: LauncherData<'_>,
        artwork: Option<Artwork<'_>>,
        typography: Option<LauncherTypography<'_>>,
        cache: Option<&mut LauncherFaceCache>,
        asset_generation: u64,
        mut load: Option<&mut dyn FnMut(usize) -> LauncherArtwork>,
    ) -> Self {
        #[cfg(feature = "launcher-profile")]
        let _preparation = crate::launcher_profile::span("prepare.launcher_constructor");
        let keys: Vec<_> = data.cards.iter().map(CardFaceKey::from).collect();
        let artwork_kind = if load.is_some() {
            2
        } else {
            match artwork {
                None => 0,
                Some(Artwork::Rgb565(_)) => 1,
                Some(Artwork::Rgb888(_)) => 2,
            }
        };
        let reusable = cache.as_ref().filter(|cache| {
            cache.scene == Some(scene)
                && cache.asset_generation == asset_generation
                && cache.slides == data.level.slides()
                && cache.artwork_kind == artwork_kind
        });
        let responsive = responsive::Layout::for_level(scene, data.level.slides());
        let fonts = responsive.map(|layout| layout.fonts(typography));
        // Every output bakes its faces through the same functions, at the size
        // it shows them: 180x252 on the HDMI landscape canvas, the card's own
        // size elsewhere, with the output's label fonts.
        let (face_width, face_height) =
            responsive.map_or((180, 252), |layout| (layout.card_w, layout.card_h));
        let (face_typography, narrow_title) =
            match fonts.as_ref().map(responsive::Fonts::for_labels) {
                Some((labels, narrow)) => (Some(labels), narrow),
                None => (typography, None),
            };
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
                    && (!cache.faces[index].source_retry || !cache.retry_artwork)
                {
                    return Arc::clone(&cache.faces[index]);
                }
                let mut loaded = load.as_mut().map(|load| load(index));
                // A retry that failed again has identical fallback pixels. Keep
                // its prepared surfaces so the UI can discard an unchanged result.
                if let Some(cache) = reusable
                    && cache.keys.get(index) == Some(&keys[index])
                    && cache.faces[index].source_retry
                    && loaded.as_ref().is_some_and(|source| source.retry)
                {
                    return Arc::clone(&cache.faces[index]);
                }

                let name = if loaded.as_ref().is_some_and(|source| {
                    source.contains_name
                        && (source.prepared.is_some() || source.pixels.len() == 360 * 504 * 3)
                }) {
                    ""
                } else {
                    card.name
                };
                // Prepared artwork is baked at 180x252; other sizes bake from the source.
                let prepared_art = ((face_width, face_height) == (180, 252))
                    .then(|| loaded.as_mut().and_then(|source| source.prepared.take()))
                    .flatten();
                let card = PreparedCard {
                    id: card.id,
                    name,
                    games: card.games,
                    colour: card.colour,
                    name_mask: text_mask(name),
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
                    rgb888: loaded
                        .as_ref()
                        .map(|source| source.pixels.as_ref())
                        .or_else(|| {
                            artwork.and_then(|items| match items {
                                Artwork::Rgb888(items) => items.get(index).copied(),
                                Artwork::Rgb565(_) => None,
                            })
                        })
                        .filter(|pixels| pixels.len() == 360 * 504 * 3),
                };
                #[cfg(test)]
                FACE_BAKES.set(FACE_BAKES.get() + 2);
                let mut faces = if let Some(prepared) = prepared_art {
                    let [compact, detail] = prepared.faces(&card, typography);
                    CardFaces {
                        source_retry: false,
                        compact,
                        detail,
                        back: None,
                        slides: data.level.slides(),
                    }
                } else if card.rgb888.is_some() {
                    #[cfg(feature = "launcher-profile")]
                    let _faces = crate::launcher_profile::span("prepare.rgb888_faces");
                    let [compact, detail] = artwork::faces_rgb888(
                        &card,
                        face_width,
                        face_height,
                        face_typography,
                        narrow_title,
                    );
                    CardFaces {
                        source_retry: false,
                        compact,
                        detail,
                        back: None,
                        slides: data.level.slides(),
                    }
                } else {
                    #[cfg(feature = "launcher-profile")]
                    let _faces = crate::launcher_profile::span("prepare.initial_faces");
                    CardFaces {
                        source_retry: false,
                        compact: bake_face(
                            &card,
                            face_width,
                            face_height,
                            false,
                            face_typography,
                            narrow_title,
                            &mut bodies,
                        ),
                        detail: bake_face(
                            &card,
                            face_width,
                            face_height,
                            true,
                            face_typography,
                            narrow_title,
                            &mut bodies,
                        ),
                        back: bodies.back_face(&card),
                        slides: data.level.slides(),
                    }
                };
                faces.source_retry = loaded.as_ref().is_some_and(|source| source.retry);
                // Every projection is dithered. A reflection is 64 rows under a
                // 252-row card; a smaller card keeps the same proportion.
                let fade_rows =
                    responsive.map_or(64, |layout| (64 * layout.card_h / 252).clamp(2, 64));
                for face in [&mut faces.compact, &mut faces.detail]
                    .into_iter()
                    .chain(faces.back.as_mut())
                {
                    face.dither = Dither::Always;
                    face.reflection_fade_rows = fade_rows;
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
            cache.retry_artwork = false;
        }
        #[cfg(feature = "launcher-profile")]
        let _buffers = crate::launcher_profile::span("prepare.retained_buffers");
        let mut prepared = Self {
            scene,
            responsive,
            chrome: chrome.clone(),
            chrome_state: ChromeState::new(data),
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
            retry_artwork: faces.iter().any(|face| face.source_retry),
            faces: Arc::new(faces),
            flip_columns: (0..if data.level.slides() {
                CAROUSEL_CAPACITY
            } else {
                6
            })
                .map(|_| {
                    crate::launcher_flip::Scratch::strip_for(if responsive.is_some() {
                        scene.width
                    } else {
                        LOGICAL_WIDTH
                    })
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
            // The damage region contains only the pure-black background in
            // chrome. Avoid reading a second framebuffer just to clear.
            clear_card_rows(
                &mut self.logical,
                LOGICAL_WIDTH,
                CardRow {
                    rows: (rect.y0, rect.y1),
                    clip: (rect.x0, rect.x1),
                },
            );
        }
        #[cfg(feature = "launcher-profile")]
        drop(clear_profile);
        if self.faces.is_empty() {
            self.fit_output();
            return;
        }
        let plan = build_carousel_plan(&self.faces, frame, self.cyclic);
        let row = CardRow::canvas(false).with_clip((self.carousel_clip().0, CardRow::RIGHT));
        draw_card_strips(
            &mut self.logical,
            LOGICAL_WIDTH,
            row,
            &plan,
            &mut self.flip_columns,
        );
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
            y0: CardRow::ROWS.0,
            x1: CardRow::RIGHT,
            y1: CardRow::ROWS.1,
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
        clip: CardRow::canvas(false).clip,
        body_clip: CardRow::canvas(false).clip,
        vertical_clip: (CardRow::ROWS.0, 438, CardRow::ROWS.1),
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
                pose.body_clip.0 =
                    ((edge - 8).max(0) as usize).clamp(CardRow::LEFT, CardRow::RIGHT);
            } else {
                pose.body_clip.1 =
                    ((edge + 8).max(0) as usize).clamp(CardRow::LEFT, CardRow::RIGHT);
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

/// The part of an output the card row owns: the rows it clears and draws in,
/// and the columns it is clipped to. Everything the carousel draws (projected
/// edges and reflections included) stays inside it, so the chrome around the row
/// never needs restoring. The 960x540 canvas has constants for it; the
/// responsive layout derives one from its margins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CardRow {
    pub rows: (usize, usize),
    pub clip: (usize, usize),
}

impl CardRow {
    /// The 960x540 canvas: the rows of the card row...
    pub const ROWS: (usize, usize) = (120, 495);
    /// ...the column where the carousel ends on the right...
    pub const RIGHT: usize = 934;
    /// ...and where it begins on the left, further left when cards slide
    /// between slots (nested levels and the level trick).
    pub const LEFT: usize = 296;
    pub const LEFT_SLIDING: usize = 268;

    /// The canvas card row for a level that does or does not slide its cards.
    pub const fn canvas(sliding: bool) -> Self {
        Self {
            rows: Self::ROWS,
            clip: (
                if sliding {
                    Self::LEFT_SLIDING
                } else {
                    Self::LEFT
                },
                Self::RIGHT,
            ),
        }
    }

    /// The same rows, narrowed to one tile's columns.
    pub const fn with_clip(self, clip: (usize, usize)) -> Self {
        Self { clip, ..self }
    }
}

/// Clear the card row to the background.
fn clear_card_rows(pixels: &mut [Rgb565Pixel], stride: usize, row: CardRow) {
    for y in row.rows.0..row.rows.1 {
        pixels[y * stride + row.clip.0..y * stride + row.clip.1].fill(Rgb565Pixel(BACKGROUND));
    }
}

/// Draw `plan` across the card row's columns in independent strips: each strip
/// finishes every reflection before its bodies, then reuses the same
/// cache-local scratch for the next. The one place every layout composes its
/// card row.
fn draw_card_strips(
    pixels: &mut [Rgb565Pixel],
    stride: usize,
    row: CardRow,
    plan: &CarouselPlan<'_>,
    scratch: &mut [crate::launcher_flip::Scratch],
) {
    let clip = row.clip;
    let width = crate::launcher_flip::STRIP_WIDTH;
    for left in (clip.0..clip.1).step_by(width) {
        draw_carousel_plan(
            pixels,
            stride,
            (0, 0),
            plan,
            scratch,
            (left, (left + width).min(clip.1)),
        );
    }
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

    #[test]
    fn root_reflection_has_no_brightness_pop_at_rest() {
        let artwork = vec![Rgb565Pixel(0xc618); 180 * 252];
        let images = [artwork.as_slice(); CARDS.len()];
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        for (direction, selected, target) in [
            (BrowseDirection::Right, 0, 1),
            (BrowseDirection::Left, 1, 0),
        ] {
            let mut prepared = LauncherScene::new(960, 540)
                .prepare_initial_with_artwork(data(), &images)
                .finish();
            let mut previous = Vec::new();
            for progress in [units - 36, units - 6, units - 2, units - 1, units] {
                prepared.render_frame(BrowseFrame {
                    selected,
                    target,
                    phase: crate::launcher_navigation::BrowsePhase::Flipping,
                    direction: Some(direction),
                    progress_millis: progress,
                    duration_millis: units,
                });
                // The centre card is within 0.08 pixels of rest. Its interior
                // reflection may retain small filtering differences, but not
                // a simultaneous colour-step change across the faded surface.
                let reflection: Vec<_> = (418..470)
                    .flat_map(|y| {
                        prepared.pixels()[y * 960 + 530..y * 960 + 700]
                            .iter()
                            .copied()
                    })
                    .collect();
                if !previous.is_empty() {
                    let changed = reflection
                        .iter()
                        .zip(&previous)
                        .filter(|(a, b)| a != b)
                        .count();
                    assert!(
                        changed < reflection.len() / 32,
                        "{direction:?} progress={progress}: {changed} reflection pixels changed"
                    );
                }
                previous = reflection;
            }
        }
    }

    #[test]
    fn embedded_wordmark_omits_title_without_losing_game_count() {
        use std::borrow::Cow;
        let scene = LauncherScene::new(960, 540);
        let mut cache = LauncherFaceCache::default();
        let mut actual = scene
            .prepare_initial_with_rgb888_loader_and_cache(
                data(),
                &mut |_| LauncherArtwork {
                    prepared: None,
                    pixels: Cow::Owned(vec![80; 360 * 504 * 3]),
                    retry: false,
                    contains_name: true,
                },
                None,
                &mut cache,
                1,
            )
            .finish();
        let mut cards = CARDS;
        for card in &mut cards {
            card.name = "";
        }
        let mut expected_data = data();
        expected_data.cards = &cards;
        let mut expected = scene
            .prepare_initial_with_rgb888_loader_and_cache(
                expected_data,
                &mut |_| LauncherArtwork {
                    prepared: None,
                    pixels: Cow::Owned(vec![80; 360 * 504 * 3]),
                    retry: false,
                    contains_name: false,
                },
                None,
                &mut LauncherFaceCache::default(),
                1,
            )
            .finish();
        let frame = BrowseFrame {
            selected: 0,
            target: 0,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        };
        actual.render_frame(frame);
        expected.render_frame(frame);
        assert_eq!(actual.pixels(), expected.pixels());
    }

    #[test]
    fn lazy_artwork_loads_only_misses_retries_failures_and_uses_generation() {
        use std::{borrow::Cow, cell::RefCell};
        let calls = RefCell::new(vec![0; CARDS.len()]);
        let failing = std::cell::Cell::new(true);
        let mut load = |i: usize| {
            calls.borrow_mut()[i] += 1;
            LauncherArtwork {
                prepared: None,
                pixels: if i == 1 && failing.get() {
                    Cow::Borrowed(&[])
                } else {
                    Cow::Owned(vec![80 + i as u8; 360 * 504 * 3])
                },
                retry: i == 1 && failing.get(),
                contains_name: false,
            }
        };
        let scene = LauncherScene::new(960, 540);
        let mut cache = LauncherFaceCache::default();
        let first = scene
            .prepare_initial_with_rgb888_loader_and_cache(data(), &mut load, None, &mut cache, 1)
            .finish();
        assert!(first.needs_artwork_retry());
        assert_eq!(*calls.borrow(), vec![1; CARDS.len()]);
        let unchanged = scene
            .prepare_initial_with_rgb888_loader_and_cache(data(), &mut load, None, &mut cache, 1)
            .finish();
        assert!(first.shares_faces_with(&unchanged));
        assert_eq!(*calls.borrow(), vec![1; CARDS.len()]);
        cache.retry_failed_artwork();
        let failed_again = scene
            .prepare_initial_with_rgb888_loader_and_cache(data(), &mut load, None, &mut cache, 1)
            .finish();
        assert!(first.shares_faces_with(&failed_again));
        assert_eq!(*calls.borrow(), vec![1, 2, 1, 1, 1]);
        failing.set(false);
        cache.retry_failed_artwork();
        let recovered = scene
            .prepare_initial_with_rgb888_loader_and_cache(data(), &mut load, None, &mut cache, 1)
            .finish();
        assert!(!recovered.needs_artwork_retry());
        assert_eq!(*calls.borrow(), vec![1, 3, 1, 1, 1]);
        let mut cards = CARDS;
        cards[2].games = Some(314);
        let mut input = data();
        input.cards = &cards;
        input.clock = "new clock";
        input.selected = 3;
        scene
            .prepare_initial_with_rgb888_loader_and_cache(input, &mut load, None, &mut cache, 1)
            .finish();
        assert_eq!(*calls.borrow(), vec![1, 3, 2, 1, 1]);
        scene
            .prepare_initial_with_rgb888_loader_and_cache(input, &mut load, None, &mut cache, 1)
            .finish();
        assert_eq!(*calls.borrow(), vec![1, 3, 2, 1, 1]);
        scene
            .prepare_initial_with_rgb888_loader_and_cache(input, &mut load, None, &mut cache, 2)
            .finish();
        assert_eq!(*calls.borrow(), vec![2, 4, 3, 2, 2]);
    }

    /// Projection, lighting, face swaps and reflections with the clean-background
    /// root renders. Reviewed at HDMI/CRT sizes; update only with intentional art
    /// or renderer changes, never to hide a motion regression.
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
        #[cfg(not(any(feature = "card-axis-filter", feature = "card-fast-quantisation")))]
        #[rustfmt::skip]
        const REFERENCE: [u64; 40] = [
            0x4687b9f98929fc27, 0x01de40663697711d, 0xf8cd84aad96c6ec2, 0x510785231b4c077d, 0xf154ac04f8082d34,
            0xf154ac04f8082d34, 0xdfa4872b2baf7994, 0x2d94a95f2e285101, 0xcaeb9d49054907c3, 0x4687b9f98929fc27,
            0xe99463293f8d6ae2, 0x564a35e20b961e2d, 0x267f4b8dcb919ebd, 0x414c148c37f63f71, 0xf2ccc280bbf36cfa,
            0xf2ccc280bbf36cfa, 0x9fd8e7e99706e952, 0x4584ab3931057503, 0x1a7154ae62e74a7d, 0xe99463293f8d6ae2,
            0x9064bd26dce17d47, 0x312220ae87347d6e, 0x0bd66dec7c94a8dc, 0x0616276d53daba0a, 0x5b2cdea94ccecf7a,
            0x5b2cdea94ccecf7a, 0x33f801ee5fa976eb, 0x8b28f03532112732, 0x0e0a87d26cc74e66, 0x9064bd26dce17d47,
            0x8f68b472aa3890a0, 0x98037568ba59fdf8, 0x2c716babb4b63a3c, 0x258c48ad6c9f74f7, 0x4db838d0ab8e4be3,
            0x4db838d0ab8e4be3, 0xafd72ebf31d1ad7c, 0xc8d54eb5720afeb2, 0x4eb7c3993079ed64, 0x8f68b472aa3890a0,
        ];
        #[cfg(all(feature = "card-axis-filter", not(feature = "card-fast-quantisation")))]
        #[rustfmt::skip]
        const REFERENCE: [u64; 40] = [
            0x4687b9f98929fc27, 0x9d2be33a3ca71867, 0x52234dbf6ae11548, 0x72d7e978eb358360, 0xf154ac04f8082d34,
            0xf154ac04f8082d34, 0x00885c42b0b1224e, 0xb8afd0c2d7cf3b77, 0x094557929b2959f5, 0x4687b9f98929fc27,
            0xe99463293f8d6ae2, 0xa12c6a59ad59d5c5, 0xc485f7ac5045d525, 0xf058a08feea6c577, 0xf2ccc280bbf36cfa,
            0xf2ccc280bbf36cfa, 0xe1c0093eff8a2114, 0x1bbad37721146d83, 0x1df9684e815143fd, 0xe99463293f8d6ae2,
            0x9064bd26dce17d47, 0x6504054bb10d2765, 0x2061e5b738a161a0, 0x7048e440e0210583, 0x5b2cdea94ccecf7a,
            0x5b2cdea94ccecf7a, 0x3ee432dabb8d656d, 0x9ab590c5b233746e, 0x1fee56dd9c38daf4, 0x9064bd26dce17d47,
            0x8f68b472aa3890a0, 0xd31e177812699575, 0x262299ee1f39bfe0, 0x63382864e33ebf07, 0x4db838d0ab8e4be3,
            0x4db838d0ab8e4be3, 0xf5c3986808fd75f8, 0x522c06cf6c7f826a, 0xf7c97440c1623415, 0x8f68b472aa3890a0,
        ];
        #[cfg(all(not(feature = "card-axis-filter"), feature = "card-fast-quantisation"))]
        #[rustfmt::skip]
        const REFERENCE: [u64; 40] = [
            0xd3ce5764e8c88bde, 0xa7cef90c802d6414, 0xea9b95bdbad042bb, 0x67a9abee891b4c1a, 0x7e1400988e2a7fb4,
            0x7e1400988e2a7fb4, 0x2cca0ac9cf164e40, 0xa430e84ae034b4bd, 0xb60e7727de597315, 0xd3ce5764e8c88bde,
            0xf857ed54ba3197d1, 0xe80277c83234ca1d, 0xb35d0cee37503fad, 0x5f1ce79ae45a81ee, 0x593208980032c68a,
            0x593208980032c68a, 0xa9dd25c205405f42, 0xcba0ac78327f8008, 0x3b1441eaf526eb3d, 0xf857ed54ba3197d1,
            0x0d162b5d41ffb7cf, 0xbf075ebb0df506cd, 0xd6fb447e35048072, 0x42f4c49b32b51bc2, 0x055fa747e041aabb,
            0x055fa747e041aabb, 0xab1aabb9523a21f4, 0xe85cfc0a60a6cba7, 0xc61620e1efb7b99b, 0x0d162b5d41ffb7cf,
            0x7c07676125de1e3a, 0x5cb0febd8ace99f9, 0x03265385f5225f79, 0x8da7684e673e44f2, 0x52bc777c57f6c6bf,
            0x52bc777c57f6c6bf, 0x9d4a57586bf76f44, 0xd01ca8f8d250bfa4, 0x5cfb444cd0fc482f, 0x7c07676125de1e3a,
        ];
        #[cfg(all(feature = "card-axis-filter", feature = "card-fast-quantisation"))]
        #[rustfmt::skip]
        const REFERENCE: [u64; 40] = [
            0xd3ce5764e8c88bde, 0xbe353139c21b115f, 0xefb5b4c5a8c033ac, 0x1dffc79049677eb6, 0x7e1400988e2a7fb4,
            0x7e1400988e2a7fb4, 0x223c8ad98be9529c, 0xc477be9f19c4cc32, 0xd9ec89c562a0692a, 0xd3ce5764e8c88bde,
            0xf857ed54ba3197d1, 0xb8f8aedfd9631dbb, 0x195fee40db73d2e7, 0x7a1f9d819a794843, 0x593208980032c68a,
            0x593208980032c68a, 0x088e75a6257fdde7, 0xdb82e968aa77ff1e, 0x4d8f392d9a180acb, 0xf857ed54ba3197d1,
            0x0d162b5d41ffb7cf, 0xaeccaedb31be72b0, 0x5a2f29cd9ec89f68, 0x43cf31d67db9e1ff, 0x055fa747e041aabb,
            0x055fa747e041aabb, 0x8f4aa1ea10dfd475, 0x031dbfafafcf0bb9, 0x18a5bc21ff031e12, 0x0d162b5d41ffb7cf,
            0x7c07676125de1e3a, 0x99f92d47bc72d3e6, 0x0540c6969f31789b, 0x5fc89ae4d72e3f2e, 0x52bc777c57f6c6bf,
            0x52bc777c57f6c6bf, 0xe1a41d06f2e0cc40, 0xf0f4dbb270b77a46, 0x917460c393fde7c8, 0x7c07676125de1e3a,
        ];
        assert_eq!(actual, REFERENCE, "Root raster: {actual:x?}");
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
    fn every_scene_dithers_every_projection() {
        for (scene, expected) in [
            (LauncherScene::new(960, 540), Dither::Always),
            (LauncherScene::new(540, 960), Dither::Always),
            (LauncherScene::crt(640, 240), Dither::Always),
            (LauncherScene::crt(240, 640), Dither::Always),
        ] {
            let prepared = PreparedLauncher::new(scene, data(), None, None);
            for faces in prepared.faces.iter() {
                for face in [&faces.compact, &faces.detail]
                    .into_iter()
                    .chain(faces.back.as_ref())
                {
                    assert_eq!(face.dither, expected, "{}x{}", scene.width, scene.height);
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

    /// Every output size and level the launcher shows, rendered at rest and
    /// mid-flip in both directions, as one 64-bit hash each. This pins the card
    /// row raster across refactors of how it is composed: any visual change
    /// must update the table on purpose. The pixel-changing experiment features
    /// (`card-axis-filter`, `card-fast-quantisation`) have their own output.
    #[cfg(not(any(feature = "card-axis-filter", feature = "card-fast-quantisation")))]
    fn card_row_hashes() -> Vec<(String, u64)> {
        fn fnv(pixels: &[Rgb565Pixel]) -> u64 {
            pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, pixel| {
                (hash ^ u64::from(pixel.0)).wrapping_mul(0x0000_0100_0000_01b3)
            })
        }
        let nested = NestedLevel {
            path: &["CONSOLES"],
            games: 60,
            children: 6,
            children_label: "MAKERS",
            detail: Some((9, "SYSTEMS")),
            accent: 0x2a7f,
        };
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        let mut out = Vec::new();
        for (name, scene) in [
            ("hdmi-960x540", LauncherScene::new(960, 540)),
            ("hdmi-1280x720", LauncherScene::new(1280, 720)),
            ("hdmi-portrait-540x960", LauncherScene::new(540, 960)),
            ("crt-640x480", LauncherScene::crt(640, 480)),
            ("crt-640x288", LauncherScene::crt(640, 288)),
            ("crt-portrait-480x640", LauncherScene::crt(480, 640)),
        ] {
            for (level_name, level) in [
                ("root", LauncherLevel::Root),
                ("nested", LauncherLevel::Nested(nested)),
            ] {
                let mut input = data();
                input.level = level;
                let mut prepared = scene.prepare(input);
                let frames = [
                    (
                        "rest",
                        0,
                        0,
                        crate::launcher_navigation::BrowsePhase::Settled,
                        None,
                        0,
                    ),
                    (
                        "right-start",
                        0,
                        1,
                        crate::launcher_navigation::BrowsePhase::Flipping,
                        Some(BrowseDirection::Right),
                        units / 8,
                    ),
                    (
                        "right-mid",
                        0,
                        1,
                        crate::launcher_navigation::BrowsePhase::Flipping,
                        Some(BrowseDirection::Right),
                        units / 2,
                    ),
                    (
                        "right-late",
                        0,
                        1,
                        crate::launcher_navigation::BrowsePhase::Flipping,
                        Some(BrowseDirection::Right),
                        units * 7 / 8,
                    ),
                    (
                        "left-mid",
                        2,
                        1,
                        crate::launcher_navigation::BrowsePhase::Flipping,
                        Some(BrowseDirection::Left),
                        units / 2,
                    ),
                ];
                for (frame_name, selected, target, phase, direction, progress) in frames {
                    prepared.render_frame(BrowseFrame {
                        selected,
                        target,
                        phase,
                        direction,
                        progress_millis: progress,
                        duration_millis: units,
                    });
                    out.push((
                        format!("{name} {level_name} {frame_name}"),
                        fnv(prepared.pixels()),
                    ));
                }
            }
        }
        out
    }

    #[cfg(not(any(feature = "card-axis-filter", feature = "card-fast-quantisation")))]
    #[test]
    fn card_row_raster_hashes_are_pinned() {
        const CARD_ROW_HASHES: [(&str, u64); 60] = [
            ("hdmi-960x540 root rest", 0x8fa7aa1c72be7616),
            ("hdmi-960x540 root right-start", 0x4e7141ecbcffefff),
            ("hdmi-960x540 root right-mid", 0xbcb05aec3e4efc4d),
            ("hdmi-960x540 root right-late", 0x07f00626f24bc90e),
            ("hdmi-960x540 root left-mid", 0xc243faf56d053520),
            ("hdmi-960x540 nested rest", 0xce41ffd8cf978135),
            ("hdmi-960x540 nested right-start", 0x4850d84b3df4321f),
            ("hdmi-960x540 nested right-mid", 0xeacef394cce29e2c),
            ("hdmi-960x540 nested right-late", 0x506de6d80f8b7ca6),
            ("hdmi-960x540 nested left-mid", 0xdaeaa223f122a241),
            ("hdmi-1280x720 root rest", 0xd6f6da8e278bb12b),
            ("hdmi-1280x720 root right-start", 0x297208a338a23511),
            ("hdmi-1280x720 root right-mid", 0xb79dc917607ccc44),
            ("hdmi-1280x720 root right-late", 0xe13e0e60cfe7482b),
            ("hdmi-1280x720 root left-mid", 0xb0a88eb3a27e5d5a),
            ("hdmi-1280x720 nested rest", 0x538ec366ae9e734f),
            ("hdmi-1280x720 nested right-start", 0xe19437bb8cd4cde2),
            ("hdmi-1280x720 nested right-mid", 0x16a61476e10090f2),
            ("hdmi-1280x720 nested right-late", 0xedc3687816f6ae9f),
            ("hdmi-1280x720 nested left-mid", 0x80966bbb01371184),
            ("hdmi-portrait-540x960 root rest", 0x2ee5bea1a8e68f4b),
            ("hdmi-portrait-540x960 root right-start", 0x464aed8b929fe268),
            ("hdmi-portrait-540x960 root right-mid", 0xa9019ddb7b4694ef),
            ("hdmi-portrait-540x960 root right-late", 0x6b7da01699d77091),
            ("hdmi-portrait-540x960 root left-mid", 0x535b6256572a01b0),
            ("hdmi-portrait-540x960 nested rest", 0xe1b58d93e857eacf),
            (
                "hdmi-portrait-540x960 nested right-start",
                0xf09452c138e7b819,
            ),
            ("hdmi-portrait-540x960 nested right-mid", 0xa6b9f8bf5acfb628),
            (
                "hdmi-portrait-540x960 nested right-late",
                0x8f42dad3c9e34e7b,
            ),
            ("hdmi-portrait-540x960 nested left-mid", 0x249d8a53803e8980),
            ("crt-640x480 root rest", 0x61c15b1f521339d7),
            ("crt-640x480 root right-start", 0x09c7d31d3dec06ce),
            ("crt-640x480 root right-mid", 0x460e7e4c166cdb2a),
            ("crt-640x480 root right-late", 0x3fcc6cca7f55c15b),
            ("crt-640x480 root left-mid", 0x457de7051d1abacf),
            ("crt-640x480 nested rest", 0x56a7fe6d90f67f56),
            ("crt-640x480 nested right-start", 0x139a9c3f638e43d3),
            ("crt-640x480 nested right-mid", 0xb6b9edd1257f7f33),
            ("crt-640x480 nested right-late", 0xcdfbd974b812204c),
            ("crt-640x480 nested left-mid", 0xd41a2651f94761c5),
            ("crt-640x288 root rest", 0x526379536bf3c9c0),
            ("crt-640x288 root right-start", 0xd8c55f60a2eb321c),
            ("crt-640x288 root right-mid", 0x7bf2ffca9101ec28),
            ("crt-640x288 root right-late", 0x42cd9f30d7935726),
            ("crt-640x288 root left-mid", 0xe0e82b047820ebbd),
            ("crt-640x288 nested rest", 0x12a295bc4f022cd3),
            ("crt-640x288 nested right-start", 0x50581ca938cdfe98),
            ("crt-640x288 nested right-mid", 0x441b4be30fdd5f96),
            ("crt-640x288 nested right-late", 0x132cfcbdb386eaba),
            ("crt-640x288 nested left-mid", 0x9993db1357110755),
            ("crt-portrait-480x640 root rest", 0xff2d6141a61ffdfa),
            ("crt-portrait-480x640 root right-start", 0xf123f1d6d117979b),
            ("crt-portrait-480x640 root right-mid", 0xfd62eb9da1524ea1),
            ("crt-portrait-480x640 root right-late", 0x763a277cc538178a),
            ("crt-portrait-480x640 root left-mid", 0x43982818f49ab168),
            ("crt-portrait-480x640 nested rest", 0x3913b8891d58a49e),
            (
                "crt-portrait-480x640 nested right-start",
                0xc8eb6cbde6bece7d,
            ),
            ("crt-portrait-480x640 nested right-mid", 0xa98170ccce29978d),
            ("crt-portrait-480x640 nested right-late", 0xde159d55105031f0),
            ("crt-portrait-480x640 nested left-mid", 0xb487d5d069649c03),
        ];
        let actual = card_row_hashes();
        let changed: Vec<_> = actual
            .iter()
            .zip(CARD_ROW_HASHES)
            .filter(|((name, hash), (pinned_name, pinned))| name != pinned_name || hash != pinned)
            .map(|((name, hash), _)| format!("(\"{name}\", {hash:#018x}),"))
            .collect();
        assert_eq!(actual.len(), CARD_ROW_HASHES.len());
        assert!(
            changed.is_empty(),
            "the card row raster changed; if that is intended, update the table:\n{}",
            changed.join("\n")
        );
    }

    /// The tile renderer (the production path for HDMI landscape) and the
    /// whole-frame renderer compose the card row through the same functions; the
    /// rows and columns the card row owns must agree for every frame and every
    /// way of splitting the carousel into tiles.
    #[test]
    fn tile_rendering_matches_frame_rendering_in_the_card_row() {
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        let mut prepared = LauncherScene::new(960, 540).prepare(data());
        let preparer = prepared.frame_preparer();
        let (left, right) = prepared.carousel_clip();
        for (selected, target, direction, progress) in [
            (0, 0, None, 0),
            (0, 1, Some(BrowseDirection::Right), units / 8),
            (0, 1, Some(BrowseDirection::Right), units / 2),
            (2, 1, Some(BrowseDirection::Left), units / 2),
            (2, 1, Some(BrowseDirection::Left), units * 7 / 8),
        ] {
            let frame = BrowseFrame {
                selected,
                target,
                phase: if direction.is_some() {
                    crate::launcher_navigation::BrowsePhase::Flipping
                } else {
                    crate::launcher_navigation::BrowsePhase::Settled
                },
                direction,
                progress_millis: progress,
                duration_millis: units,
            };
            prepared.render_frame(frame);
            let expected = prepared.pixels().to_vec();
            let request = LauncherFrameRequest {
                frame,
                timestamp_us: 0,
                generation: 1,
            };
            for splits in [
                vec![(left, right)],
                vec![(left, 600), (600, right)],
                vec![(left, 400), (400, 700), (700, right)],
            ] {
                let mut buffer = preparer.new_tile_buffer();
                let mut tiled = vec![Rgb565Pixel(0x1234); LOGICAL_WIDTH * LOGICAL_HEIGHT];
                for clip in &splits {
                    preparer.render_tile_into(request, &mut buffer, &mut tiled, *clip);
                }
                for y in 120..495 {
                    let row = y * LOGICAL_WIDTH;
                    assert!(
                        tiled[row + left..row + right] == expected[row + left..row + right],
                        "{frame:?} splits {splits:?} row {y}"
                    );
                }
            }
        }
    }

    /// Writes the launcher home screen of every output size, at rest and
    /// mid-flip, as PPM files so a change to the card renderer can be looked at.
    /// `MAGIK_CARD_PREVIEW_DIR` is the output directory; `MAGIK_CARD_ART_DIR`
    /// holds 360x504 RGB888 artwork (`*.rgb888`), and without it the plain
    /// fallback faces are drawn. Run with:
    /// `MAGIK_CARD_PREVIEW_DIR=/tmp/cards cargo test --lib write_card_previews -- --ignored`
    #[test]
    #[ignore = "writes image files; set MAGIK_CARD_PREVIEW_DIR"]
    fn write_card_previews() {
        let dir = std::env::var("MAGIK_CARD_PREVIEW_DIR").expect("MAGIK_CARD_PREVIEW_DIR");
        let art: Vec<Vec<u8>> = std::env::var("MAGIK_CARD_ART_DIR")
            .ok()
            .map(|dir| {
                let mut names: Vec<_> = std::fs::read_dir(dir)
                    .unwrap()
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| path.extension().is_some_and(|ext| ext == "rgb888"))
                    .collect();
                names.sort();
                names
                    .into_iter()
                    .map(|path| std::fs::read(path).unwrap())
                    .filter(|bytes| bytes.len() == 360 * 504 * 3)
                    .collect()
            })
            .unwrap_or_default();
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        for (name, scene) in [
            ("hdmi-landscape-960x540", LauncherScene::new(960, 540)),
            ("hdmi-portrait-540x960", LauncherScene::new(540, 960)),
            ("crt-landscape-640x480", LauncherScene::crt(640, 480)),
            ("crt-landscape-640x288", LauncherScene::crt(640, 288)),
            ("crt-portrait-480x640", LauncherScene::crt(480, 640)),
            ("crt-portrait-288x640", LauncherScene::crt(288, 640)),
        ] {
            let images: Vec<&[u8]> = art.iter().map(Vec::as_slice).take(CARDS.len()).collect();
            let mut prepared = if images.len() == CARDS.len() {
                scene
                    .prepare_initial_with_rgb888_artwork(data(), &images)
                    .finish()
            } else {
                scene.prepare(data())
            };
            for (frame_name, frame) in [
                (
                    "rest",
                    BrowseFrame {
                        selected: 0,
                        target: 0,
                        phase: crate::launcher_navigation::BrowsePhase::Settled,
                        direction: None,
                        progress_millis: 0,
                        duration_millis: units,
                    },
                ),
                (
                    "flip",
                    BrowseFrame {
                        selected: 0,
                        target: 1,
                        phase: crate::launcher_navigation::BrowsePhase::Flipping,
                        direction: Some(BrowseDirection::Right),
                        progress_millis: units / 2,
                        duration_millis: units,
                    },
                ),
            ] {
                prepared.render_frame(frame);
                let mut ppm = format!("P6\n{} {}\n255\n", scene.width, scene.height).into_bytes();
                for pixel in prepared.pixels() {
                    let v = pixel.0;
                    ppm.push((((v >> 11) & 31) as u8) << 3 | ((v >> 13) & 7) as u8);
                    ppm.push((((v >> 5) & 63) as u8) << 2 | ((v >> 9) & 3) as u8);
                    ppm.push(((v & 31) as u8) << 3 | ((v >> 2) & 7) as u8);
                }
                std::fs::write(format!("{dir}/{name}-{frame_name}.ppm"), ppm).unwrap();
            }
        }
    }

    #[test]
    fn every_card_bakes_at_the_size_each_output_shows_it() {
        let source: Vec<u8> = (0..360 * 504 * 3)
            .map(|i| ((i * 7 + i / 360) % 251) as u8)
            .collect();
        let ids = [
            LauncherCardId::Arcade,
            LauncherCardId::Consoles,
            LauncherCardId::Computers,
            LauncherCardId::Handhelds,
            LauncherCardId::Favourites,
            LauncherCardId::Settings,
        ];
        let cards: Vec<LauncherCard<'static>> = ids
            .iter()
            .map(|&id| LauncherCard {
                id,
                name: "SUPER NINTENDO",
                games: Some(1234),
                colour: LauncherCardStyle::root(id).colour,
            })
            .collect();
        let images: Vec<&[u8]> = vec![source.as_slice(); cards.len()];
        let nested = NestedLevel {
            path: &["CONSOLES"],
            games: 60,
            children: 6,
            children_label: "MAKERS",
            detail: None,
            accent: 0x2a7f,
        };
        for scene in [
            LauncherScene::new(960, 540),
            LauncherScene::new(540, 960),
            LauncherScene::crt(640, 480),
            LauncherScene::crt(640, 240),
            LauncherScene::crt(640, 288),
            LauncherScene::crt(480, 640),
            LauncherScene::crt(240, 640),
            LauncherScene::crt(288, 640),
        ] {
            for level in [LauncherLevel::Root, LauncherLevel::Nested(nested)] {
                let (width, height) = responsive::Layout::for_level(scene, level.slides())
                    .map_or((180, 252), |layout| (layout.card_w, layout.card_h));
                for artwork in [None, Some(Artwork::Rgb888(&images))] {
                    let mut input = data();
                    input.cards = &cards;
                    input.level = level;
                    let prepared = PreparedLauncher::new(scene, input, artwork, None);
                    for faces in prepared.faces.iter() {
                        for face in [&faces.compact, &faces.detail] {
                            assert_eq!(
                                (face.width, face.height),
                                (width, height),
                                "{}x{} {:?}",
                                scene.width,
                                scene.height,
                                artwork.is_some()
                            );
                        }
                    }
                }
            }
        }
    }

    /// Time `render_frame` across a full flip and at rest for each output, in
    /// microseconds per frame. Run in release with real artwork:
    /// `MAGIK_CARD_ART_DIR=... cargo test --release --lib bench_card_row_render -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark; prints timings"]
    fn bench_card_row_render() {
        let art: Vec<Vec<u8>> = std::env::var("MAGIK_CARD_ART_DIR")
            .ok()
            .map(|dir| {
                let mut names: Vec<_> = std::fs::read_dir(dir)
                    .unwrap()
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| path.extension().is_some_and(|ext| ext == "rgb888"))
                    .collect();
                names.sort();
                names
                    .into_iter()
                    .map(|path| std::fs::read(path).unwrap())
                    .filter(|bytes| bytes.len() == 360 * 504 * 3)
                    .collect()
            })
            .unwrap_or_default();
        let units = crate::launcher_navigation::SPRING_POSITION_UNITS;
        for (name, scene) in [
            ("hdmi-960x540", LauncherScene::new(960, 540)),
            ("hdmi-portrait-540x960", LauncherScene::new(540, 960)),
            ("crt-640x480", LauncherScene::crt(640, 480)),
            ("crt-640x240", LauncherScene::crt(640, 240)),
            ("crt-640x288", LauncherScene::crt(640, 288)),
            ("crt-portrait-480x640", LauncherScene::crt(480, 640)),
        ] {
            let images: Vec<&[u8]> = art.iter().map(Vec::as_slice).take(CARDS.len()).collect();
            let mut prepared = if images.len() == CARDS.len() {
                scene
                    .prepare_initial_with_rgb888_artwork(data(), &images)
                    .finish()
            } else {
                scene.prepare(data())
            };
            let frames: Vec<BrowseFrame> = (0..=30)
                .map(|i| BrowseFrame {
                    selected: 0,
                    target: 1,
                    phase: crate::launcher_navigation::BrowsePhase::Flipping,
                    direction: Some(BrowseDirection::Right),
                    progress_millis: units * i / 30,
                    duration_millis: units,
                })
                .collect();
            for frame in &frames {
                prepared.render_frame(*frame);
            }
            let mut flip = f64::MAX;
            for _ in 0..6 {
                let started = std::time::Instant::now();
                for frame in &frames {
                    prepared.render_frame(*frame);
                }
                flip = flip.min(started.elapsed().as_secs_f64() * 1e6 / frames.len() as f64);
            }
            let rest = BrowseFrame {
                selected: 0,
                target: 0,
                phase: crate::launcher_navigation::BrowsePhase::Settled,
                direction: None,
                progress_millis: 0,
                duration_millis: units,
            };
            let started = std::time::Instant::now();
            for _ in 0..30 {
                prepared.render_frame(rest);
            }
            let rest_us = started.elapsed().as_secs_f64() * 1e6 / 30.0;
            println!("BENCH {name}: flip {flip:.0} us/frame, rest {rest_us:.0} us/frame");
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
