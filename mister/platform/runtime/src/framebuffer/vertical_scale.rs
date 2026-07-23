use super::downsample::Rgb565FrameView;
use super::{DirtyRect, Rgb565Pixel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalRgb565Transform {
    width: usize,
    source_height: usize,
    destination_height: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalCopyStats {
    pub destination_rect: DirtyRect,
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
            .and_then(|pixels| pixels.checked_mul(std::mem::size_of::<Rgb565Pixel>()))
            .ok_or("vertical transform source geometry overflow")?;
        width
            .checked_mul(destination_height)
            .and_then(|pixels| pixels.checked_mul(std::mem::size_of::<Rgb565Pixel>()))
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
        let numerator =
            (destination_y.checked_mul(2)?.checked_add(1)?).checked_mul(self.source_height)?;
        Some((numerator / (self.destination_height * 2)).min(self.source_height - 1))
    }

    pub fn destination_rect_for_source(self, source_rect: DirtyRect) -> Option<DirtyRect> {
        let source_rect = self.valid_source_rect(source_rect)?;
        let first_destination_y = (0..self.destination_height).find(|&y| {
            self.source_row_for_destination(y)
                .is_some_and(|sy| sy >= source_rect.y)
        })?;
        let destination_y_end = (first_destination_y..self.destination_height)
            .find(|&y| {
                self.source_row_for_destination(y)
                    .is_some_and(|sy| sy >= source_rect.y + source_rect.height)
            })
            .unwrap_or(self.destination_height);

        Some(DirtyRect::new(
            source_rect.x,
            first_destination_y,
            source_rect.width,
            destination_y_end - first_destination_y,
        ))
    }

    pub fn copy_rect(
        self,
        source: Rgb565FrameView<'_>,
        source_rect: DirtyRect,
        destination: &mut [Rgb565Pixel],
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

        for destination_y in destination_rect.y..destination_rect.y + destination_rect.height {
            let source_y = self
                .source_row_for_destination(destination_y)
                .ok_or("destination row is outside vertical transform geometry")?;
            let source_offset = source_y * source.stride_pixels + source_rect.x;
            let destination_offset = destination_y * destination_stride + source_rect.x;
            destination[destination_offset..destination_offset + source_rect.width]
                .copy_from_slice(&source.pixels[source_offset..source_offset + source_rect.width]);
        }

        Ok(Some(VerticalCopyStats {
            destination_rect,
            bytes: destination_rect.width
                * destination_rect.height
                * std::mem::size_of::<Rgb565Pixel>(),
        }))
    }

    fn valid_source_rect(self, source_rect: DirtyRect) -> Option<DirtyRect> {
        let x_end = source_rect.x.checked_add(source_rect.width)?;
        let y_end = source_rect.y.checked_add(source_rect.height)?;
        if source_rect.width == 0
            || source_rect.height == 0
            || x_end > self.width
            || y_end > self.source_height
        {
            return None;
        }
        Some(source_rect)
    }

    fn validate_source(self, source: Rgb565FrameView<'_>) -> Result<(), &'static str> {
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
                transform.destination_rect_for_source(DirtyRect::new(0, 0, 640, 480)),
                Some(DirtyRect::new(0, 0, 640, height))
            );
        }
    }

    #[test]
    fn unsampled_source_rows_produce_no_destination_damage() {
        let transform = transform(240);
        assert_eq!(
            transform.destination_rect_for_source(DirtyRect::new(0, 0, 640, 1)),
            None
        );
        assert_eq!(
            transform.destination_rect_for_source(DirtyRect::new(0, 1, 640, 1)),
            Some(DirtyRect::new(0, 0, 640, 1))
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
            .copy_rect(view, DirtyRect::new(1, 0, 2, 4), &mut destination, 6)
            .unwrap()
            .unwrap();

        assert_eq!(
            stats,
            VerticalCopyStats {
                destination_rect: DirtyRect::new(1, 0, 2, 2),
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
        assert!(transform
            .copy_rect(view, DirtyRect::new(3, 0, 2, 1), &mut destination, 4,)
            .unwrap()
            .is_none());
        assert!(transform
            .copy_rect(view, DirtyRect::new(0, 0, 4, 4), &mut destination, 3,)
            .is_err());
    }
}
