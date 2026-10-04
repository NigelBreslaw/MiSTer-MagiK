// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exact nearest-neighbour coordinates without division in the pixel loop.

#[derive(Clone, Copy, Debug)]
pub struct NearestAxis {
    index: usize,
    step: usize,
    error: usize,
    remainder: usize,
    denominator: usize,
    remaining: usize,
}

impl NearestAxis {
    /// Produce floor(x * source / destination) for x in start..start + count.
    /// Clipped ranges retain the exact phase of the original, unclipped image.
    pub fn new(source: usize, destination: usize, start: usize, count: usize) -> Option<Self> {
        if source == 0 || destination == 0 || start.checked_add(count)? > destination {
            return None;
        }
        let initial = start as u128 * source as u128;
        Some(Self {
            index: (initial / destination as u128) as usize,
            error: (initial % destination as u128) as usize,
            step: source / destination,
            remainder: source % destination,
            denominator: destination,
            remaining: count,
        })
    }
}

impl Iterator for NearestAxis {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        if self.remaining == 0 {
            return None;
        }
        let index = self.index;
        self.remaining -= 1;
        if self.remaining != 0 {
            self.index += self.step;
            // Subtraction avoids overflow even for extents near usize::MAX.
            let until_carry = self.denominator - self.remainder;
            if self.error >= until_carry {
                self.error -= until_carry;
                self.index += 1;
            } else {
                self.error += self.remainder;
            }
        }
        Some(index)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for NearestAxis {}
impl std::iter::FusedIterator for NearestAxis {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_sequences_match_integer_division_for_up_down_and_identity_scaling() {
        for source in 1..70 {
            for destination in 1..70 {
                for start in 0..=destination {
                    let axis =
                        NearestAxis::new(source, destination, start, destination - start).unwrap();
                    let expected = (start..destination).map(|x| x * source / destination);
                    assert!(
                        axis.eq(expected),
                        "source={source} destination={destination} start={start}"
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_and_large_extents_are_bounded_without_overflow() {
        assert!(NearestAxis::new(0, 1, 0, 1).is_none());
        assert!(NearestAxis::new(1, 0, 0, 0).is_none());
        assert!(NearestAxis::new(1, 5, 4, 2).is_none());
        assert!(NearestAxis::new(1, usize::MAX, usize::MAX, 1).is_none());
        for source in [1, usize::MAX / 2, usize::MAX] {
            let destination = usize::MAX - 3;
            let start = destination - 9;
            let actual = NearestAxis::new(source, destination, start, 9).unwrap();
            let expected = (start..destination)
                .map(|x| (x as u128 * source as u128 / destination as u128) as usize);
            assert!(actual.eq(expected));
        }
    }
}
