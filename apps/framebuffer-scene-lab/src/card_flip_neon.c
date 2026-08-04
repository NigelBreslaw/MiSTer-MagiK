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

void mister_magik_card_flip_neon_fill_rect(
    uint16_t *destination,
    size_t stride,
    size_t x,
    size_t y,
    size_t width,
    size_t height,
    uint16_t value
) {
    for (size_t row = 0; row < height; ++row) {
        mister_magik_card_flip_neon_fill(
            destination + (y + row) * stride + x,
            value,
            width
        );
    }
}
