// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

// Keep this as a stable, inspectable symbol: the lab verifies the linked ARM
// instructions before trusting device timing evidence.
__attribute__((noinline)) void mister_magik_hidden_copy_rgb565_neon(
    uint16_t *restrict destination,
    const uint16_t *restrict source,
    size_t pixels) {
    size_t index = 0;
    for (; index + 32 <= pixels; index += 32) {
        const uint16x8_t a = vld1q_u16(source + index);
        const uint16x8_t b = vld1q_u16(source + index + 8);
        const uint16x8_t c = vld1q_u16(source + index + 16);
        const uint16x8_t d = vld1q_u16(source + index + 24);
        vst1q_u16(destination + index, a);
        vst1q_u16(destination + index + 8, b);
        vst1q_u16(destination + index + 16, c);
        vst1q_u16(destination + index + 24, d);
    }
    for (; index + 8 <= pixels; index += 8) {
        vst1q_u16(destination + index, vld1q_u16(source + index));
    }
    for (; index < pixels; ++index) {
        destination[index] = source[index];
    }
}
