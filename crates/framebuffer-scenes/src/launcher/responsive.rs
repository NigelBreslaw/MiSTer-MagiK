// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native-raster launcher layouts. Only artwork is filtered; bitmap labels are
//! baked at the selected card's exact output dimensions, never screen-scaled.
use super::*;
use crate::bitmap_text::BitmapGlyph;
use std::borrow::Cow;

// CRT roles share one scaled font; HDMI borrows the supplied role fonts.
pub(super) enum Fonts<'a> {
    /// The route's scaled cell, plus the same font at native width for
    /// labels that would not fit the doubled cell.
    Uniform(Cow<'a, BitmapFont>, Cow<'a, BitmapFont>),
    Roles(LauncherTypography<'a>),
}

impl Fonts<'_> {
    /// A card title that fits `width`: the heading, or a narrower face.
    fn card_title(&self, text: &str, width: usize) -> &BitmapFont {
        let heading = self.get(TextRole::Heading);
        if heading.measure(text) <= width {
            return heading;
        }
        match self {
            Self::Uniform(_, narrow) => narrow,
            Self::Roles(fonts) => fonts.font_for(TextRole::Metadata, text),
        }
    }

    fn get(&self, role: TextRole) -> &BitmapFont {
        match self {
            Self::Uniform(font, _) => font,
            Self::Roles(fonts) => match role {
                TextRole::Heading => fonts.heading,
                TextRole::Number => fonts.number,
                TextRole::Metadata => fonts.metadata,
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    width: usize,
    height: usize,
    crt: bool,
    sx: usize,
    sy: usize,
    margin_x: usize,
    margin_y: usize,
    top: usize,
    bottom: usize,
    pub card_h: usize,
    card_w: usize,
    centre_y: usize,
    library_y: usize,
}

impl Layout {
    pub fn for_scene(scene: LauncherScene) -> Option<Self> {
        if !scene.crt && scene.width >= scene.height {
            return None;
        }
        let (width, height) = (scene.width, scene.height);
        // Native 15 kHz scanout has twice as many horizontal samples. Rotate
        // the sampling axes with the logical canvas, not the physical monitor.
        let narrow = width.min(height);
        let (sx, sy) = if scene.crt {
            if narrow <= 288 && width.max(height) >= 640 {
                if width > height { (2, 1) } else { (1, 2) }
            } else if narrow >= 400 {
                (2, 2)
            } else {
                (1, 1)
            }
        } else {
            (1, 1)
        };
        // PAL 288-line scanout is displayed at the same 480-line inspection
        // height as 240p. Correct artwork proportions independently from the
        // integer bitmap-font grid (fractional font scaling would blur text).
        let (aspect_y, aspect_x) = match (scene.crt, width, height) {
            (true, 640, 288) => (3, 5),
            (true, 288, 640) => (5, 3),
            _ => (sy, sx),
        };
        let margin_x = (width * 6 / 100).max(8 * sx).max(scene.safe_insets.0);
        let margin_y = (height * 5 / 100).max(6 * sy).max(scene.safe_insets.1);
        let top = margin_y + if scene.crt { 36 * sy } else { 110 };
        let library_y = height * 56 / 100;
        let bottom = if scene.crt {
            height.saturating_sub(margin_y + 34 * sy)
        } else {
            library_y.saturating_sub(26)
        };
        let available_h = bottom.saturating_sub(top);
        let available_w = width.saturating_sub(2 * margin_x);
        let card_w =
            ((available_w * 34 / 100).min(available_h * 5 * aspect_x / (9 * aspect_y)) / 2 * 2)
                .max(72 * sx);
        let card_h = (card_w * 7 * aspect_y / (5 * aspect_x) / 2 * 2).max(2);
        let centre_y = top + available_h.saturating_sub(card_h + card_h / 5) / 2 + card_h / 2;
        Some(Self {
            width,
            height,
            crt: scene.crt,
            sx,
            sy,
            margin_x,
            margin_y,
            top,
            bottom,
            card_h,
            card_w,
            centre_y,
            library_y,
        })
    }

    pub fn fonts<'a>(&self, typography: Option<LauncherTypography<'a>>) -> Fonts<'a> {
        if let Some(fonts) = typography.filter(|_| !self.crt) {
            return Fonts::Roles(fonts);
        }
        let font = typography
            .map(|fonts| Cow::Borrowed(fonts.fallback))
            .unwrap_or_else(|| Cow::Owned(legacy_font()));
        let narrow = if self.sy == 1 {
            font.clone()
        } else {
            Cow::Owned(scale_font(&font, 1, self.sy))
        };
        Fonts::Uniform(
            if (self.sx, self.sy) == (1, 1) {
                font
            } else {
                Cow::Owned(scale_font(&font, self.sx, self.sy))
            },
            narrow,
        )
    }

    pub fn chrome(&self, pixels: &mut [Rgb565Pixel], data: LauncherData<'_>, fonts: &Fonts<'_>) {
        pixels.fill(Rgb565Pixel(BACKGROUND));
        let heading = fonts.get(TextRole::Heading);
        let metadata = fonts.get(TextRole::Metadata);
        let left = self.margin_x;
        let right = self.width.saturating_sub(left);
        let draw = |pixels: &mut [Rgb565Pixel],
                    font: &BitmapFont,
                    x: usize,
                    y: usize,
                    text: &str,
                    colour| {
            font.draw(
                pixels,
                self.width,
                self.height,
                x as i32,
                y as i32,
                text,
                colour,
            );
        };
        if let LauncherLevel::Nested(level) = data.level {
            // `CONSOLES / NINTENDO`: ancestors muted, the current group in cream.
            let mut x = left;
            for (index, name) in level.path.iter().enumerate() {
                let last = index + 1 == level.path.len();
                draw(
                    pixels,
                    heading,
                    x,
                    self.margin_y,
                    name,
                    if last { CREAM } else { MUTED },
                );
                x += heading.measure(name);
                if !last {
                    draw(pixels, heading, x, self.margin_y, " / ", MUTED);
                    x += heading.measure(" / ");
                }
            }
        } else {
            draw(pixels, heading, left, self.margin_y, "MISTER MAGIK", CREAM);
        }
        draw(
            pixels,
            heading,
            right.saturating_sub(heading.measure(data.clock)),
            self.margin_y,
            data.clock,
            CREAM,
        );
        let header_bottom = self.margin_y + if self.crt { 18 * self.sy } else { 46 };
        self.rule(pixels, header_bottom);
        let section = match data.level {
            LauncherLevel::Root => "COLLECTIONS",
            LauncherLevel::Nested(level) => level.children_label,
        };
        draw(
            pixels,
            metadata,
            left,
            header_bottom + 8 * self.sy,
            section,
            MUTED,
        );
        if let LauncherLevel::Nested(level) = data.level {
            // CRT has no sidebar: the group total sits opposite the section.
            let total = format_games(level.games);
            draw(
                pixels,
                metadata,
                right.saturating_sub(metadata.measure(&total)),
                header_bottom + 8 * self.sy,
                &total,
                MUTED,
            );
        }
        let footer_rule = self.height.saturating_sub(self.margin_y + 20 * self.sy);
        self.rule(pixels, footer_rule);
        draw(
            pixels,
            metadata,
            left,
            footer_rule + 5 * self.sy,
            "A OPEN",
            CREAM,
        );
        draw(
            pixels,
            metadata,
            left + metadata.measure("A OPEN") + 18 * self.sx,
            footer_rule + 5 * self.sy,
            "B BACK",
            CREAM,
        );
        // Arrow glyphs vary between the production fonts; ASCII remains clear
        // on every route and is also controller-direction agnostic.
        let browse = "<  BROWSE CARDS  >";
        draw(
            pixels,
            metadata,
            self.width.saturating_sub(metadata.measure(browse)) / 2,
            self.bottom + 3 * self.sy,
            browse,
            MUTED,
        );
        if self.crt {
            return;
        }
        let number = fonts.get(TextRole::Number);
        self.rule(pixels, self.library_y);
        let y = self.library_y + 24;
        let (title, games, children, children_label, favourites) = match data.level {
            LauncherLevel::Root => (
                "YOUR LIBRARY",
                data.library_games,
                data.collections,
                "COLLECTIONS",
                data.favourites,
            ),
            LauncherLevel::Nested(level) => (
                level.path.last().copied().unwrap_or(""),
                level.games,
                level.children,
                level.children_label,
                level.favourites,
            ),
        };
        draw(pixels, metadata, left, y, title, MUTED);
        draw(pixels, number, left, y + 38, &games.to_string(), CREAM);
        draw(pixels, metadata, left, y + 90, "GAMES READY TO PLAY", MUTED);
        let counts_y = y + (footer_rule.saturating_sub(y) * 48 / 100);
        self.rule(pixels, counts_y - 16);
        draw(pixels, number, left, counts_y, &children.to_string(), CREAM);
        draw(
            pixels,
            number,
            self.width / 2,
            counts_y,
            &favourites.to_string(),
            CREAM,
        );
        draw(pixels, metadata, left, counts_y + 48, children_label, MUTED);
        draw(
            pixels,
            metadata,
            self.width / 2,
            counts_y + 48,
            "FAVOURITES",
            MUTED,
        );
        if let LauncherLevel::Nested(level) = data.level {
            for y in footer_rule.saturating_sub(38)..footer_rule.saturating_sub(31) {
                pixels[y * self.width + left..y * self.width + right]
                    .fill(Rgb565Pixel(level.accent));
            }
            return;
        }
        let bar_w = (right - left) / 4;
        for (index, colour) in [
            rgb(226, 52, 67),
            rgb(237, 193, 54),
            rgb(85, 170, 91),
            rgb(41, 145, 196),
        ]
        .into_iter()
        .enumerate()
        {
            for y in footer_rule.saturating_sub(38)..footer_rule.saturating_sub(31) {
                let x = left + index * bar_w;
                pixels[y * self.width + x..y * self.width + x + bar_w.saturating_sub(12)]
                    .fill(Rgb565Pixel(colour));
            }
        }
    }

    fn rule(&self, pixels: &mut [Rgb565Pixel], y: usize) {
        if y < self.height {
            pixels[y * self.width + self.margin_x..(y + 1) * self.width - self.margin_x]
                .fill(Rgb565Pixel(RULE));
        }
    }

    pub fn faces(&self, card: &PreparedCard<'_>, fonts: &Fonts<'_>) -> CardFaces {
        let (w, h) = (self.card_w, self.card_h);
        let (mut pixels, alpha) = artwork::native_surface(card, w, h);
        let title = fonts.card_title(card.name, w.saturating_sub(8 * self.sx));
        let metadata = fonts.get(TextRole::Metadata);
        let detail_y = if self.crt {
            (h * 86 / 100).min(h.saturating_sub(14 * self.sy))
        } else {
            h * 86 / 100
        };
        let title_y = if self.crt {
            (h * 73 / 100).min(detail_y.saturating_sub(14 * self.sy))
        } else {
            h * 73 / 100
        };
        title.draw_centered(
            &mut pixels,
            w,
            h,
            (w / 2) as i32,
            title_y as i32,
            card.name,
            CREAM,
        );
        let compact = crate::launcher_flip::Face::with_alpha(pixels.clone(), &alpha, w, h);
        if let Some(count) = card.games {
            metadata.draw_centered(
                &mut pixels,
                w,
                h,
                (w / 2) as i32,
                detail_y as i32,
                &format_games(count),
                CREAM,
            );
        }
        CardFaces {
            compact,
            detail: crate::launcher_flip::Face::with_alpha(pixels, &alpha, w, h),
        }
    }

    pub fn render(
        &self,
        pixels: &mut [Rgb565Pixel],
        faces: &[CardFaces],
        frame: BrowseFrame,
        cyclic: bool,
        scratch: &mut [crate::launcher_flip::Scratch],
    ) {
        let clip = (self.margin_x, self.width - self.margin_x);
        for y in self.top..self.bottom {
            pixels[y * self.width + clip.0..y * self.width + clip.1].fill(Rgb565Pixel(BACKGROUND));
        }
        if faces.is_empty() {
            return;
        }
        let mut plan = build_carousel_plan(faces, frame, cyclic);
        self.map_plan(&mut plan);
        self.draw_plan(pixels, &plan, scratch);
    }

    /// Carousel rows owned by the card renderer: cleared before every frame.
    pub fn clear_carousel(&self, pixels: &mut [Rgb565Pixel]) {
        let clip = (self.margin_x, self.width - self.margin_x);
        for y in self.top..self.bottom {
            pixels[y * self.width + clip.0..y * self.width + clip.1].fill(Rgb565Pixel(BACKGROUND));
        }
    }

    /// Map landscape-logical poses to this route's card geometry. Reuse the
    /// same navigation, flip, occlusion and reflection contract; only the
    /// route-owned card geometry changes.
    pub fn map_plan(&self, plan: &mut CarouselPlan<'_>) {
        let clip = (self.margin_x, self.width - self.margin_x);
        let near = self.card_w as i64 * 4 / 5;
        let far = (self.width - 2 * self.margin_x) as i64 / 2 - self.card_w as i64 * 31 / 100;
        let offset = |x: i64| {
            let distance = x.abs();
            let mapped = if distance <= 144 * GEOMETRY_ONE {
                distance * near / 144
            } else {
                near * GEOMETRY_ONE + (distance - 144 * GEOMETRY_ONE) * (far - near) / 110
            };
            x.signum() * mapped
        };
        for item in plan.items.iter_mut().flatten() {
            let old = item.pose;
            let centre = self.width as i64 * GEOMETRY_ONE / 2
                + offset(old.x + old.width / 2 - 610 * GEOMETRY_ONE);
            let width = old.width * self.card_w as i64 / 180;
            let height = old.height * self.card_h as i64 / 252;
            // Vertical displacement from the resting row scales with the card.
            let lift = (old.top + old.height / 2 - 284 * GEOMETRY_ONE) * self.card_h as i64 / 252;
            item.pose.x = centre - width / 2;
            item.pose.top = self.centre_y as i64 * GEOMETRY_ONE + lift - height / 2;
            item.pose.width = width;
            item.pose.height = height;
            item.pose.clip = clip;
            item.pose.body_clip = clip;
            item.pose.vertical_clip = (self.top, self.bottom, self.bottom);
        }
    }

    pub fn draw_plan(
        &self,
        pixels: &mut [Rgb565Pixel],
        plan: &CarouselPlan<'_>,
        scratch: &mut [crate::launcher_flip::Scratch],
    ) {
        let clip = (self.margin_x, self.width - self.margin_x);
        for left in (clip.0..clip.1).step_by(crate::launcher_flip::STRIP_WIDTH) {
            draw_carousel_plan(
                pixels,
                self.width,
                (0, 0),
                plan,
                scratch,
                (left, (left + crate::launcher_flip::STRIP_WIDTH).min(clip.1)),
            );
        }
    }

    /// Chrome rows that differ between hierarchy levels: the header with the
    /// breadcrumb and section label, and the portrait group summary.
    pub fn level_chrome_rows(&self) -> [(usize, usize); 2] {
        let summary = if self.crt {
            (0, 0)
        } else {
            (self.library_y, self.height)
        };
        [(0, self.top), summary]
    }
}

fn scale_font(font: &BitmapFont, sx: usize, sy: usize) -> BitmapFont {
    BitmapFont {
        ascent: font.ascent * sy as i32,
        descent: font.descent * sy as i32,
        glyphs: font
            .glyphs
            .iter()
            .map(|g| BitmapGlyph {
                code_point: g.code_point,
                left: g.left * sx as i32,
                top: g.top * sy as i32,
                width: g.width * sx,
                height: g.height * sy,
                advance: g.advance * sx as i32,
                alpha: (0..g.height * sy)
                    .flat_map(|y| {
                        (0..g.width * sx).map(move |x| g.alpha[(y / sy) * g.width + x / sx])
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn legacy_font() -> BitmapFont {
    BitmapFont {
        ascent: 7,
        descent: 0,
        glyphs: (' '..='~')
            .map(|code_point| {
                let mask = glyph(code_point);
                BitmapGlyph {
                    code_point,
                    left: 0,
                    top: 7,
                    width: 5,
                    height: 7,
                    advance: 6,
                    alpha: mask
                        .into_iter()
                        .flat_map(|row| {
                            (0..5).map(move |x| if row & (1 << (4 - x)) != 0 { 255 } else { 0 })
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_faces_are_native_pixel_aligned_and_respect_safe_margins() {
        for (w, h) in [
            (640, 240),
            (240, 640),
            (640, 288),
            (288, 640),
            (640, 480),
            (480, 640),
            (640, 512),
            (512, 640),
            (640, 576),
            (576, 640),
        ] {
            let scene = LauncherScene::crt(w, h).with_safe_content(crate::Rgb565Rect {
                x0: 20,
                y0: 24,
                x1: w - 32,
                y1: h - 20,
            });
            let layout = Layout::for_scene(scene).unwrap();
            assert!(layout.margin_x >= 32 && layout.margin_y >= 24);
            assert_eq!(layout.card_w % 2, 0);
            assert_eq!(layout.card_h % 2, 0);
            assert!(layout.top < layout.centre_y - layout.card_h / 2);
            assert!(layout.centre_y + layout.card_h / 2 < layout.bottom);
            // Every selected label has an unbroken 1:1 bitmap projection.
            let fonts = layout.fonts(None);
            let font = fonts.get(TextRole::Heading);
            for title in [
                "ARCADE",
                "CONSOLES",
                "COMPUTERS",
                "HANDHELDS",
                "FAVOURITES",
                "SETTINGS",
            ] {
                assert!(font.measure(title) <= layout.card_w - 6, "{w}x{h}: {title}");
            }
        }
    }

    #[test]
    fn resting_crt_labels_reach_the_output_without_resampling() {
        let cards = [LauncherCard {
            id: LauncherCardId::Favourites,
            name: "FAVOURITES",
            games: Some(35216),
            colour: 0xf81f,
        }];
        for (w, h) in [
            (640, 240),
            (240, 640),
            (640, 288),
            (288, 640),
            (640, 480),
            (480, 640),
            (640, 512),
            (512, 640),
        ] {
            let scene = LauncherScene::crt(w, h);
            let layout = Layout::for_scene(scene).unwrap();
            let mut prepared = scene.prepare(LauncherData {
                cards: &cards,
                selected: 0,
                library_games: 35216,
                collections: 77,
                favourites: 1,
                clock: "07:28",
                level: LauncherLevel::Root,
            });
            prepared.render_frame(BrowseFrame {
                selected: 0,
                target: 0,
                phase: crate::launcher_navigation::BrowsePhase::Settled,
                direction: None,
                progress_millis: 0,
                duration_millis: 0,
            });
            let face = &prepared.faces[0].detail;
            let (x0, y0) = ((w - layout.card_w) / 2, layout.centre_y - layout.card_h / 2);
            for y in layout.card_h * 73 / 100..layout.card_h * 95 / 100 {
                for x in 6..layout.card_w - 6 {
                    assert_eq!(
                        prepared.pixels()[(y0 + y) * w + x0 + x],
                        face.pixels[y * layout.card_w + x],
                        "{w}x{h} label pixel {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn crt_omits_library_statistics_but_hdmi_portrait_keeps_them() {
        let cards = [];
        let data = LauncherData {
            cards: &cards,
            selected: 0,
            library_games: 35216,
            collections: 77,
            favourites: 1,
            clock: "07:28",
            level: LauncherLevel::Root,
        };
        let changed = LauncherData {
            library_games: 42,
            collections: 3,
            favourites: 99,
            ..data
        };
        for scene in [
            LauncherScene::crt(640, 240),
            LauncherScene::crt(240, 640),
            LauncherScene::crt(640, 512),
            LauncherScene::new(540, 960),
        ] {
            let a = scene.render(data);
            let b = scene.render(changed);
            assert_eq!(a == b, scene.crt);
        }
    }
}
