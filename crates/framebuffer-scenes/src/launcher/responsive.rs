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
    /// The fonts the card labels are drawn with, and the narrower title font
    /// for names the cell cannot fit, when this output has one.
    pub(super) fn for_labels(&self) -> (LauncherTypography<'_>, Option<&BitmapFont>) {
        match self {
            Self::Uniform(font, narrow) => (
                LauncherTypography {
                    heading: font,
                    number: font,
                    metadata: font,
                    fallback: font,
                },
                Some(narrow),
            ),
            Self::Roles(fonts) => (*fonts, None),
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
    pub card_w: usize,
    centre_y: usize,
    library_y: usize,
}

impl Layout {
    pub fn for_level(scene: LauncherScene, nested: bool) -> Option<Self> {
        let mut layout = Self::for_scene(scene)?;
        if nested && layout.crt && layout.width > layout.height {
            let (ax, ay) = if (layout.width, layout.height) == (640, 288) {
                (5, 3)
            } else {
                (layout.sx, layout.sy)
            };
            // Derive width from the native height so the row retains 5:7 after
            // pixel-aspect correction. Root geometry remains exactly as prepared.
            layout.card_w = layout.card_h * 5 * ax / (7 * ay) / 2 * 2;
        }
        Some(layout)
    }

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
        let (title, games, children, children_label, detail) = match data.level {
            LauncherLevel::Root => (
                "YOUR LIBRARY",
                data.library_games,
                data.collections,
                "COLLECTIONS",
                Some((data.favourites, "FAVOURITES")),
            ),
            LauncherLevel::Nested(level) => (
                level.path.last().copied().unwrap_or(""),
                level.games,
                level.children,
                level.children_label,
                level.detail,
            ),
        };
        draw(pixels, metadata, left, y, title, MUTED);
        draw(pixels, number, left, y + 38, &games.to_string(), CREAM);
        draw(pixels, metadata, left, y + 90, "GAMES READY TO PLAY", MUTED);
        let counts_y = y + (footer_rule.saturating_sub(y) * 48 / 100);
        self.rule(pixels, counts_y - 16);
        draw(pixels, number, left, counts_y, &children.to_string(), CREAM);
        draw(pixels, metadata, left, counts_y + 48, children_label, MUTED);
        if let Some((value, label)) = detail {
            draw(
                pixels,
                number,
                self.width / 2,
                counts_y,
                &value.to_string(),
                CREAM,
            );
            draw(
                pixels,
                metadata,
                self.width / 2,
                counts_y + 48,
                label,
                MUTED,
            );
        }
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

    pub fn render(
        &self,
        pixels: &mut [Rgb565Pixel],
        faces: &[Arc<CardFaces>],
        frame: BrowseFrame,
        cyclic: bool,
        scratch: &mut [crate::launcher_flip::Scratch],
    ) {
        self.clear_carousel(pixels);
        if faces.is_empty() {
            return;
        }
        let mut plan = if self.crt && faces.first().is_some_and(|f| f.slides) {
            row::build_with_tilt(faces, frame, 0)
        } else {
            build_carousel_plan(faces, frame, cyclic)
        };
        self.map_plan(&mut plan);
        self.draw_plan(pixels, &plan, scratch);
    }

    /// Carousel rows owned by the card renderer: cleared before every frame.
    pub fn clear_carousel(&self, pixels: &mut [Rgb565Pixel]) {
        clear_card_rows(pixels, self.width, self.card_row());
    }

    /// The rows between the header and the library, across the margins.
    pub fn card_row(&self) -> CardRow {
        CardRow {
            rows: (self.top, self.bottom),
            clip: (self.margin_x, self.width - self.margin_x),
        }
    }

    /// Map landscape-logical poses to this route's card geometry. Reuse the
    /// same navigation, flip, occlusion and reflection contract; only the
    /// route-owned card geometry changes.
    pub fn map_plan(&self, plan: &mut CarouselPlan<'_>) {
        for item in plan.items.iter_mut().flatten() {
            item.pose = self.map_pose(item.pose, plan.row);
        }
    }

    pub fn map_pose(
        &self,
        old: crate::launcher_flip::Pose,
        row: bool,
    ) -> crate::launcher_flip::Pose {
        let mut pose = old;
        let clip = (self.margin_x, self.width - self.margin_x);
        let width = old.width * self.card_w as i64 / 180;
        let height = old.height * self.card_h as i64 / 252;
        if row {
            pose.x = self.margin_x as i64 * GEOMETRY_ONE
                + (old.x - 292 * GEOMETRY_ONE) * self.card_w as i64 / 180;
            pose.top = self.centre_y as i64 * GEOMETRY_ONE - height / 2;
            if self.crt {
                pose.brightness = row::crt_brightness(old.brightness);
            }
        } else {
            let near = self.card_w as i64 * 4 / 5;
            let far = (self.width - 2 * self.margin_x) as i64 / 2 - self.card_w as i64 * 31 / 100;
            let x = old.x + old.width / 2 - 610 * GEOMETRY_ONE;
            let distance = x.abs();
            let mapped = if distance <= 144 * GEOMETRY_ONE {
                distance * near / 144
            } else {
                near * GEOMETRY_ONE + (distance - 144 * GEOMETRY_ONE) * (far - near) / 110
            };
            let centre = self.width as i64 * GEOMETRY_ONE / 2 + x.signum() * mapped;
            let lift = (old.top + old.height / 2 - 284 * GEOMETRY_ONE) * self.card_h as i64 / 252;
            pose.x = centre - width / 2;
            pose.top = self.centre_y as i64 * GEOMETRY_ONE + lift - height / 2;
        }
        pose.width = width;
        pose.height = height;
        pose.clip = clip;
        pose.body_clip = clip;
        pose.vertical_clip = (self.top, self.bottom, self.bottom);
        pose
    }

    pub fn draw_plan(
        &self,
        pixels: &mut [Rgb565Pixel],
        plan: &CarouselPlan<'_>,
        scratch: &mut [crate::launcher_flip::Scratch],
    ) {
        draw_card_strips(pixels, self.width, self.card_row(), plan, scratch);
    }

    /// Chrome rows that differ between hierarchy levels: the header with the
    /// breadcrumb and section label, and the portrait group summary.
    pub fn title_rect(&self) -> (usize, usize, usize, usize) {
        (
            self.margin_x,
            0,
            self.width.saturating_sub(self.margin_x + 80 * self.sx),
            self.margin_y + if self.crt { 18 * self.sy } else { 46 },
        )
    }

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
