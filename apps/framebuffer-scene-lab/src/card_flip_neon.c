// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_card_flip_neon_fill(uint16_t *destination, uint16_t value, size_t count) {
    const uint16x8_t packed = vdupq_n_u16(value);
    size_t index = 0;

    for (; index + 32 <= count; index += 32) {
        vst1q_u16(destination + index, packed);
        vst1q_u16(destination + index + 8, packed);
        vst1q_u16(destination + index + 16, packed);
        vst1q_u16(destination + index + 24, packed);
    }
    for (; index + 8 <= count; index += 8) {
        vst1q_u16(destination + index, packed);
    }
    for (; index < count; ++index) {
        destination[index] = value;
    }
}

void mister_magik_card_flip_neon_copy(
    uint16_t *destination,
    const uint16_t *source,
    size_t count
) {
    size_t index = 0;

    for (; index + 32 <= count; index += 32) {
        const uint16x8_t pixels0 = vld1q_u16(source + index);
        const uint16x8_t pixels1 = vld1q_u16(source + index + 8);
        const uint16x8_t pixels2 = vld1q_u16(source + index + 16);
        const uint16x8_t pixels3 = vld1q_u16(source + index + 24);
        vst1q_u16(destination + index, pixels0);
        vst1q_u16(destination + index + 8, pixels1);
        vst1q_u16(destination + index + 16, pixels2);
        vst1q_u16(destination + index + 24, pixels3);
    }
    for (; index + 8 <= count; index += 8) {
        const uint16x8_t pixels = vld1q_u16(source + index);
        vst1q_u16(destination + index, pixels);
    }
    for (; index < count; ++index) {
        destination[index] = source[index];
    }
}
