// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

static inline uint16x8_t darken_rgb565_7_8_vector(
    uint16x8_t packed,
    uint16x8_t red_blue_mask,
    uint16x8_t green_mask,
    int16x8_t seven_eighths_q15
) {
    uint16x8_t red = vshrq_n_u16(packed, 11);
    uint16x8_t green = vandq_u16(vshrq_n_u16(packed, 5), green_mask);
    uint16x8_t blue = vandq_u16(packed, red_blue_mask);
    red = vreinterpretq_u16_s16(vqdmulhq_s16(
        vreinterpretq_s16_u16(red), seven_eighths_q15));
    green = vreinterpretq_u16_s16(vqdmulhq_s16(
        vreinterpretq_s16_u16(green), seven_eighths_q15));
    blue = vreinterpretq_u16_s16(vqdmulhq_s16(
        vreinterpretq_s16_u16(blue), seven_eighths_q15));
    return vorrq_u16(
        vshlq_n_u16(red, 11),
        vorrq_u16(vshlq_n_u16(green, 5), blue)
    );
}

void mister_magik_scanline_neon_darken_7_8_rows(
    uint16_t *restrict pixels,
    size_t vector_count,
    size_t rows,
    size_t stride
) {
    const uint16x8_t red_blue_mask = vdupq_n_u16(0x001f);
    const uint16x8_t green_mask = vdupq_n_u16(0x003f);
    const int16x8_t seven_eighths_q15 = vdupq_n_s16(0x7000);
    for (size_t row = 0; row < rows; row++) {
        uint16_t *row_pixels = pixels + row * stride;
        uint16_t *next_row_pixels = row + 1 < rows ? row_pixels + stride : NULL;
        size_t vector = 0;
        for (; vector + 4 <= vector_count; vector += 4) {
            if (next_row_pixels != NULL) {
                __builtin_prefetch(next_row_pixels + vector * 8, 1, 1);
            }
            uint16_t *block = row_pixels + vector * 8;
            uint16x8_t packed0 = vld1q_u16(block);
            uint16x8_t packed1 = vld1q_u16(block + 8);
            uint16x8_t packed2 = vld1q_u16(block + 16);
            uint16x8_t packed3 = vld1q_u16(block + 24);
            vst1q_u16(block, darken_rgb565_7_8_vector(
                packed0, red_blue_mask, green_mask, seven_eighths_q15));
            vst1q_u16(block + 8, darken_rgb565_7_8_vector(
                packed1, red_blue_mask, green_mask, seven_eighths_q15));
            vst1q_u16(block + 16, darken_rgb565_7_8_vector(
                packed2, red_blue_mask, green_mask, seven_eighths_q15));
            vst1q_u16(block + 24, darken_rgb565_7_8_vector(
                packed3, red_blue_mask, green_mask, seven_eighths_q15));
        }
        for (; vector < vector_count; vector++) {
            uint16_t *block = row_pixels + vector * 8;
            uint16x8_t packed = vld1q_u16(block);
            vst1q_u16(block, darken_rgb565_7_8_vector(
                packed, red_blue_mask, green_mask, seven_eighths_q15));
        }
    }
}
