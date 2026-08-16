// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl VerticalRect {
    pub const fn width(self) -> usize {
        self.x1 - self.x0
    }

    pub const fn rows(self) -> usize {
        self.y1 - self.y0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rgb565FrameView<'a, P> {
    pub pixels: &'a [P],
    pub width: usize,
    pub height: usize,
    pub stride_pixels: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalRgb565Transform {
    width: usize,
    source_height: usize,
    destination_height: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalCopyStats {
    pub destination_rect: VerticalRect,
    pub bytes: usize,
}

impl VerticalRgb565Transform {
    pub fn new(
        width: usize,
        source_height: usize,
        destination_height: usize,
    ) -> Result<Self, &'static str> {
        if width == 0 || source_height == 0 || destination_height == 0 {
            return Err("vertical transform geometry must be non-zero");
        }
        width
            .checked_mul(source_height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or("vertical transform source geometry overflow")?;
        width
            .checked_mul(destination_height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or("vertical transform destination geometry overflow")?;
        Ok(Self {
            width,
            source_height,
            destination_height,
        })
    }

    pub const fn width(self) -> usize {
        self.width
    }

    pub const fn source_height(self) -> usize {
        self.source_height
    }

    pub const fn destination_height(self) -> usize {
        self.destination_height
    }

    pub fn source_row_for_destination(self, destination_y: usize) -> Option<usize> {
        if destination_y >= self.destination_height {
            return None;
        }
        if self.source_height == self.destination_height {
            return Some(destination_y);
        }
        let numerator =
            (destination_y.checked_mul(2)?.checked_add(1)?).checked_mul(self.source_height)?;
        Some((numerator / (self.destination_height * 2)).min(self.source_height - 1))
    }

    pub fn destination_rect_for_source(self, source_rect: VerticalRect) -> Option<VerticalRect> {
        let source_rect = self.valid_source_rect(source_rect)?;
        if self.source_height == self.destination_height {
            return Some(source_rect);
        }
        let first_destination_y = (0..self.destination_height).find(|&y| {
            self.source_row_for_destination(y)
                .is_some_and(|sy| sy >= source_rect.y0 && sy < source_rect.y1)
        })?;
        let destination_y_end = (first_destination_y..self.destination_height)
            .find(|&y| {
                self.source_row_for_destination(y)
                    .is_some_and(|sy| sy >= source_rect.y1)
            })
            .unwrap_or(self.destination_height);

        Some(VerticalRect {
            x0: source_rect.x0,
            y0: first_destination_y,
            x1: source_rect.x1,
            y1: destination_y_end,
        })
    }

    pub fn copy_rect<P: Copy>(
        self,
        source: Rgb565FrameView<'_, P>,
        source_rect: VerticalRect,
        destination: &mut [P],
        destination_stride: usize,
    ) -> Result<Option<VerticalCopyStats>, &'static str> {
        self.validate_source(source)?;
        if destination_stride < self.width {
            return Err("destination stride is narrower than vertical transform width");
        }
        let required_destination_pixels = destination_stride
            .checked_mul(self.destination_height)
            .ok_or("destination geometry overflow")?;
        if destination.len() < required_destination_pixels {
            return Err("destination buffer is smaller than vertical transform geometry");
        }
        let Some(source_rect) = self.valid_source_rect(source_rect) else {
            return Ok(None);
        };
        let Some(destination_rect) = self.destination_rect_for_source(source_rect) else {
            return Ok(None);
        };

        // CRT 240p uses an exact 2:1 vertical transform. The general path
        // recomputes the source row mapping for every destination row; the
        // fixed-ratio path keeps the same centre-sampled odd source rows while
        // reducing that arithmetic on the latch hot path.
        if self.source_height == self.destination_height.saturating_mul(2)
            && source_rect.x0 == 0
            && source_rect.x1 == self.width
            && source.stride_pixels == self.width
            && destination_stride == self.width
        {
            for destination_y in destination_rect.y0..destination_rect.y1 {
                let source_y = destination_y.saturating_mul(2).saturating_add(1);
                let source_start = source_y * source.stride_pixels;
                let destination_start = destination_y * destination_stride;
                destination[destination_start..destination_start + self.width]
                    .copy_from_slice(&source.pixels[source_start..source_start + self.width]);
            }
            return Ok(Some(VerticalCopyStats {
                destination_rect,
                bytes: destination_rect.width() * destination_rect.rows() * 2,
            }));
        }

        if self.source_height == self.destination_height {
            if source_rect.x0 == 0
                && source_rect.x1 == self.width
                && source.stride_pixels == self.width
                && destination_stride == self.width
            {
                let source_start = source_rect.y0 * source.stride_pixels;
                let destination_start = source_rect.y0 * destination_stride;
                let pixel_count = source_rect.width() * source_rect.rows();
                destination[destination_start..destination_start + pixel_count]
                    .copy_from_slice(&source.pixels[source_start..source_start + pixel_count]);
            } else {
                for y in source_rect.y0..source_rect.y1 {
                    let source_offset = y * source.stride_pixels + source_rect.x0;
                    let destination_offset = y * destination_stride + source_rect.x0;
                    destination[destination_offset..destination_offset + source_rect.width()]
                        .copy_from_slice(
                            &source.pixels[source_offset..source_offset + source_rect.width()],
                        );
                }
            }
            return Ok(Some(VerticalCopyStats {
                destination_rect,
                bytes: source_rect.width() * source_rect.rows() * 2,
            }));
        }

        for destination_y in destination_rect.y0..destination_rect.y1 {
            let source_y = self
                .source_row_for_destination(destination_y)
                .ok_or("destination row is outside vertical transform geometry")?;
            let source_offset = source_y * source.stride_pixels + source_rect.x0;
            let destination_offset = destination_y * destination_stride + source_rect.x0;
            destination[destination_offset..destination_offset + source_rect.width()]
                .copy_from_slice(
                    &source.pixels[source_offset..source_offset + source_rect.width()],
                );
        }

        Ok(Some(VerticalCopyStats {
            destination_rect,
            bytes: destination_rect.width() * destination_rect.rows() * 2,
        }))
    }

    fn valid_source_rect(self, source_rect: VerticalRect) -> Option<VerticalRect> {
        if source_rect.x0 >= source_rect.x1
            || source_rect.y0 >= source_rect.y1
            || source_rect.x1 > self.width
            || source_rect.y1 > self.source_height
        {
            return None;
        }
        Some(source_rect)
    }

    fn validate_source<P>(self, source: Rgb565FrameView<'_, P>) -> Result<(), &'static str> {
        if source.width != self.width || source.height != self.source_height {
            return Err("source geometry does not match vertical transform");
        }
        if source.stride_pixels < source.width {
            return Err("source stride is narrower than source width");
        }
        let required_source_pixels = source
            .stride_pixels
            .checked_mul(source.height)
            .ok_or("source geometry overflow")?;
        if source.pixels.len() < required_source_pixels {
            return Err("source buffer is smaller than source geometry");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(destination_height: usize) -> VerticalRgb565Transform {
        VerticalRgb565Transform::new(640, 480, destination_height).unwrap()
    }

    fn rect(x: usize, y: usize, width: usize, height: usize) -> VerticalRect {
        VerticalRect {
            x0: x,
            y0: y,
            x1: x + width,
            y1: y + height,
        }
    }

    #[test]
    fn centred_nearest_row_phase_is_stable_for_all_crt_heights() {
        let expected = [
            (240, [1, 3, 5, 479]),
            (288, [0, 2, 4, 479]),
            (480, [0, 1, 2, 479]),
            (576, [0, 1, 2, 479]),
        ];

        for (height, rows) in expected {
            let transform = transform(height);
            assert_eq!(transform.source_row_for_destination(0), Some(rows[0]));
            assert_eq!(transform.source_row_for_destination(1), Some(rows[1]));
            assert_eq!(transform.source_row_for_destination(2), Some(rows[2]));
            assert_eq!(
                transform.source_row_for_destination(height - 1),
                Some(rows[3])
            );
            assert_eq!(transform.source_row_for_destination(height), None);
        }
    }

    #[test]
    fn full_source_damage_maps_to_full_destination() {
        for height in [240, 288, 480, 576] {
            let transform = transform(height);
            assert_eq!(
                transform.destination_rect_for_source(rect(0, 0, 640, 480)),
                Some(rect(0, 0, 640, height))
            );
        }
    }

    #[test]
    fn unsampled_source_rows_produce_no_destination_damage() {
        let transform = transform(240);
        assert_eq!(
            transform.destination_rect_for_source(rect(0, 0, 640, 1)),
            None
        );
        assert_eq!(
            transform.destination_rect_for_source(rect(0, 1, 640, 1)),
            Some(rect(0, 0, 640, 1))
        );
    }

    #[test]
    fn identity_mapping_preserves_rows_and_rectangles() {
        let transform = VerticalRgb565Transform::new(4, 4, 4).unwrap();
        for y in 0..4 {
            assert_eq!(transform.source_row_for_destination(y), Some(y));
        }
        assert_eq!(transform.source_row_for_destination(4), None);
        assert_eq!(
            transform.destination_rect_for_source(rect(1, 1, 2, 2)),
            Some(rect(1, 1, 2, 2))
        );
    }

    #[test]
    fn identity_dense_full_width_band_is_copied_exactly() {
        let source = (0_u16..16).collect::<Vec<_>>();
        let view = Rgb565FrameView {
            pixels: &source,
            width: 4,
            height: 4,
            stride_pixels: 4,
        };
        let mut destination = [99_u16; 16];
        let stats = VerticalRgb565Transform::new(4, 4, 4)
            .unwrap()
            .copy_rect(view, rect(0, 1, 4, 2), &mut destination, 4)
            .unwrap()
            .unwrap();

        assert_eq!(
            stats,
            VerticalCopyStats {
                destination_rect: rect(0, 1, 4, 2),
                bytes: 16,
            }
        );
        assert_eq!(
            destination,
            [99, 99, 99, 99, 4, 5, 6, 7, 8, 9, 10, 11, 99, 99, 99, 99]
        );
    }

    #[test]
    fn identity_partial_copy_preserves_pixels_and_padded_strides() {
        let source = [
            0, 1, 2, 3, 90, 91, 10, 11, 12, 13, 92, 93, 20, 21, 22, 23, 94, 95, 30, 31, 32, 33, 96,
            97,
        ];
        let view = Rgb565FrameView {
            pixels: &source,
            width: 4,
            height: 4,
            stride_pixels: 6,
        };
        let mut destination = [7; 24];
        let stats = VerticalRgb565Transform::new(4, 4, 4)
            .unwrap()
            .copy_rect(view, rect(1, 1, 2, 2), &mut destination, 6)
            .unwrap()
            .unwrap();

        assert_eq!(
            stats,
            VerticalCopyStats {
                destination_rect: rect(1, 1, 2, 2),
                bytes: 8,
            }
        );
        assert_eq!(
            destination,
            [
                7, 7, 7, 7, 7, 7, 7, 11, 12, 7, 7, 7, 7, 21, 22, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            ]
        );
    }

    #[test]
    fn partial_copy_respects_padded_strides_and_unchanged_memory() {
        let source = [
            10, 11, 12, 13, 99, 99, 20, 21, 22, 23, 99, 99, 30, 31, 32, 33, 99, 99, 40, 41, 42, 43,
            99, 99,
        ];
        let view = Rgb565FrameView {
            pixels: &source,
            width: 4,
            height: 4,
            stride_pixels: 6,
        };
        let mut destination = [7; 12];
        let stats = VerticalRgb565Transform::new(4, 4, 2)
            .unwrap()
            .copy_rect(view, rect(1, 0, 2, 4), &mut destination, 6)
            .unwrap()
            .unwrap();

        assert_eq!(
            stats,
            VerticalCopyStats {
                destination_rect: rect(1, 0, 2, 2),
                bytes: 8,
            }
        );
        assert_eq!(destination, [7, 21, 22, 7, 7, 7, 7, 41, 42, 7, 7, 7]);
    }

    #[test]
    fn invalid_geometry_and_bounds_are_rejected() {
        assert!(VerticalRgb565Transform::new(0, 480, 240).is_err());
        let source = [0; 16];
        let view = Rgb565FrameView {
            pixels: &source,
            width: 4,
            height: 4,
            stride_pixels: 4,
        };
        let mut destination = [0; 8];
        let transform = VerticalRgb565Transform::new(4, 4, 2).unwrap();
        assert!(
            transform
                .copy_rect(view, rect(3, 0, 2, 1), &mut destination, 4,)
                .unwrap()
                .is_none()
        );
        assert!(
            transform
                .copy_rect(view, rect(0, 0, 4, 4), &mut destination, 3,)
                .is_err()
        );
    }
}
