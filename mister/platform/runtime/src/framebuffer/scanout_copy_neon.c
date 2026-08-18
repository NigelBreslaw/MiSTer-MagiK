// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_scanout_copy_rgb565_rect_neon(
    uint16_t *restrict destination,
    size_t destination_stride,
    const uint16_t *restrict source,
    size_t source_stride,
    size_t width,
    size_t height) {
    for (size_t row = 0; row < height; ++row) {
        uint16_t *dst = destination + row * destination_stride;
        const uint16_t *src = source + row * source_stride;
        size_t column = 0;

        for (; column + 32 <= width; column += 32) {
            __builtin_prefetch(src + column + 64, 0, 1);
            const uint16x8_t p0 = vld1q_u16(src + column);
            const uint16x8_t p1 = vld1q_u16(src + column + 8);
            const uint16x8_t p2 = vld1q_u16(src + column + 16);
            const uint16x8_t p3 = vld1q_u16(src + column + 24);
            vst1q_u16(dst + column, p0);
            vst1q_u16(dst + column + 8, p1);
            vst1q_u16(dst + column + 16, p2);
            vst1q_u16(dst + column + 24, p3);
        }
        for (; column + 8 <= width; column += 8) {
            vst1q_u16(dst + column, vld1q_u16(src + column));
        }
        for (; column < width; ++column) {
            dst[column] = src[column];
        }
    }
}
