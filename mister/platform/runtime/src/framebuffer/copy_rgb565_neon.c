// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_copy_rgb565_neon(
    uint16_t *destination,
    const uint16_t *source,
    size_t count
) {
    const size_t block_pixels = 64;
    const size_t vector_pixels = 8;
    const size_t prefetch_pixels = 128;
    size_t index = 0;

    for (; index + block_pixels <= count; index += block_pixels) {
        if (index + prefetch_pixels < count) {
            __builtin_prefetch(source + index + prefetch_pixels, 0, 3);
        }
        const uint16x8_t q0 = vld1q_u16(source + index);
        const uint16x8_t q1 = vld1q_u16(source + index + 8);
        const uint16x8_t q2 = vld1q_u16(source + index + 16);
        const uint16x8_t q3 = vld1q_u16(source + index + 24);
        const uint16x8_t q4 = vld1q_u16(source + index + 32);
        const uint16x8_t q5 = vld1q_u16(source + index + 40);
        const uint16x8_t q6 = vld1q_u16(source + index + 48);
        const uint16x8_t q7 = vld1q_u16(source + index + 56);
        vst1q_u16(destination + index, q0);
        vst1q_u16(destination + index + 8, q1);
        vst1q_u16(destination + index + 16, q2);
        vst1q_u16(destination + index + 24, q3);
        vst1q_u16(destination + index + 32, q4);
        vst1q_u16(destination + index + 40, q5);
        vst1q_u16(destination + index + 48, q6);
        vst1q_u16(destination + index + 56, q7);
    }
    for (; index + vector_pixels <= count; index += vector_pixels) {
        vst1q_u16(destination + index, vld1q_u16(source + index));
    }
    for (; index < count; ++index) {
        destination[index] = source[index];
    }
}
