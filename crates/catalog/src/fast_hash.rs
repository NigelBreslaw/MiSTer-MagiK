// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! A fast non-cryptographic hasher for builder-only maps over our own data.
//!
//! Std's default SipHash defends against hash flooding by untrusted keys, which
//! these string tables never see, and it is slow on a Cortex-A9. This is the
//! multiply-rotate "Fx" scheme, one machine word per step.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

const SEED: usize = 0x9E37_79B9_7F4A_7C15_u64 as usize;

#[derive(Default)]
pub(crate) struct FastHasher {
    state: usize,
}

impl FastHasher {
    #[inline]
    fn add(&mut self, word: usize) {
        self.state = (self.state.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        const WORD: usize = std::mem::size_of::<usize>();
        let (words, rest) = bytes.as_chunks::<WORD>();
        for word in words {
            self.add(usize::from_ne_bytes(*word));
        }
        if !rest.is_empty() {
            let mut word = [0_u8; WORD];
            word[..rest.len()].copy_from_slice(rest);
            self.add(usize::from_ne_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(usize::from(value));
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.state as u64
    }
}

pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_strings_hash_equally_and_prefixes_differ() {
        let hash = |text: &str| {
            let mut hasher = FastHasher::default();
            std::hash::Hash::hash(text, &mut hasher);
            hasher.finish()
        };
        assert_eq!(hash("mario kart"), hash("mario kart"));
        assert_ne!(hash("mario"), hash("mario "));
        assert_ne!(hash(""), hash("a"));
    }

    #[test]
    fn map_behaves_like_a_hash_map() {
        let mut map = FastMap::<String, u32>::default();
        for index in 0..1_000 {
            map.insert(format!("game {index}"), index);
        }
        assert_eq!(map.len(), 1_000);
        assert_eq!(map["game 777"], 777);
        assert!(!map.contains_key("game 1000"));
    }
}
