// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Complete immutable tile-image identity in a pair of caller-owned slots.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TileImageIdentity {
    pub content_generation: u64,
    pub render_generation: Option<u64>,
}
impl TileImageIdentity {
    pub const fn new(content_generation: u64, render_generation: u64) -> Self {
        Self {
            content_generation,
            render_generation: Some(render_generation),
        }
    }
}
impl From<u64> for TileImageIdentity {
    fn from(content_generation: u64) -> Self {
        Self {
            content_generation,
            render_generation: None,
        }
    }
}

#[derive(Default)]
pub struct RetainedTileSlots {
    images: [Option<TileImageIdentity>; 2],
}
impl RetainedTileSlots {
    pub fn invalidate_all(&mut self) {
        self.images = [None; 2];
    }
    pub fn invalidate_slot(&mut self, slot: u8) {
        self.images[index(slot)] = None;
    }

    /// Only writes may be omitted on a hit: the caller must still publish,
    /// post and verify the frame. The caller owns writable-slot qualification,
    /// geometry/damage validity, other-writer and failed-post invalidation.
    /// Failed/partial writes leave this slot unknown. A content-only identity
    /// cannot prove the tile image and always invokes the writer.
    pub fn write_if_changed<T, E>(
        &mut self,
        slot: u8,
        identity: TileImageIdentity,
        write: impl FnOnce() -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let index = index(slot);
        if identity.render_generation.is_some() && self.images[index] == Some(identity) {
            return Ok(None);
        }
        self.images[index] = None;
        let result = write()?;
        if identity.render_generation.is_some() {
            self.images[index] = Some(identity);
        }
        Ok(Some(result))
    }
}
fn index(slot: u8) -> usize {
    assert!((1..=2).contains(&slot), "tile slot must be 1 or 2");
    usize::from(slot - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_image_and_failed_writes_cannot_leave_a_resident_key() {
        let mut slots = RetainedTileSlots::default();
        let original = TileImageIdentity::new(7, 19);
        for slot in [1, 2] {
            assert_eq!(
                slots.write_if_changed(slot, original, || Ok::<_, ()>(1)),
                Ok(Some(1))
            );
        }
        assert_eq!(
            slots.write_if_changed(1, original, || panic!("resident writer ran")),
            Ok::<Option<()>, ()>(None)
        );
        let changed = TileImageIdentity::new(7, 20);
        assert_eq!(
            slots.write_if_changed(1, changed, || Err::<(), _>("partial write")),
            Err("partial write")
        );
        assert_eq!(
            slots.write_if_changed(1, original, || Ok::<_, ()>(2)),
            Ok(Some(2))
        );
        assert_eq!(
            slots.write_if_changed(2, original, || panic!("other slot lost identity")),
            Ok::<Option<()>, ()>(None)
        );
        slots.invalidate_slot(2);
        assert_eq!(
            slots.write_if_changed(2, original, || Ok::<_, ()>(3)),
            Ok(Some(3))
        );
        let new_content = TileImageIdentity::new(8, 19);
        for slot in [1, 2] {
            assert_eq!(
                slots.write_if_changed(slot, new_content, || Ok::<_, ()>(4)),
                Ok(Some(4))
            );
        }
        slots.invalidate_all();
        for slot in [1, 2] {
            assert_eq!(
                slots.write_if_changed(slot, new_content, || Ok::<_, ()>(5)),
                Ok(Some(5))
            );
        }
    }
    #[test]
    fn unknown_render_identity_always_writes() {
        let mut slots = RetainedTileSlots::default();
        for _ in 0..8 {
            assert_eq!(
                slots.write_if_changed(1, 7.into(), || Ok::<_, ()>(1)),
                Ok(Some(1))
            );
        }
    }
}
