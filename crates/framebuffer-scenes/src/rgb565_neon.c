// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

static inline void transpose8(uint16x8_t rows[8], uint16x8_t columns[8]) {
    const uint16x8x2_t t0 = vtrnq_u16(rows[0], rows[1]);
    const uint16x8x2_t t1 = vtrnq_u16(rows[2], rows[3]);
    const uint16x8x2_t t2 = vtrnq_u16(rows[4], rows[5]);
    const uint16x8x2_t t3 = vtrnq_u16(rows[6], rows[7]);
    const uint32x4x2_t t4 = vtrnq_u32(
        vreinterpretq_u32_u16(t0.val[0]), vreinterpretq_u32_u16(t1.val[0]));
    const uint32x4x2_t t5 = vtrnq_u32(
        vreinterpretq_u32_u16(t0.val[1]), vreinterpretq_u32_u16(t1.val[1]));
    const uint32x4x2_t t6 = vtrnq_u32(
        vreinterpretq_u32_u16(t2.val[0]), vreinterpretq_u32_u16(t3.val[0]));
    const uint32x4x2_t t7 = vtrnq_u32(
        vreinterpretq_u32_u16(t2.val[1]), vreinterpretq_u32_u16(t3.val[1]));
    const uint64x2x2_t t8 = vtrnq_u64(
        vreinterpretq_u64_u32(t4.val[0]), vreinterpretq_u64_u32(t6.val[0]));
    const uint64x2x2_t t9 = vtrnq_u64(
        vreinterpretq_u64_u32(t4.val[1]), vreinterpretq_u64_u32(t6.val[1]));
    const uint64x2x2_t t10 = vtrnq_u64(
        vreinterpretq_u64_u32(t5.val[0]), vreinterpretq_u64_u32(t7.val[0]));
    const uint64x2x2_t t11 = vtrnq_u64(
        vreinterpretq_u64_u32(t5.val[1]), vreinterpretq_u64_u32(t7.val[1]));
    columns[0] = vreinterpretq_u16_u64(t8.val[0]);
    columns[1] = vreinterpretq_u16_u64(t9.val[0]);
    columns[2] = vreinterpretq_u16_u64(t8.val[1]);
    columns[3] = vreinterpretq_u16_u64(t9.val[1]);
    columns[4] = vreinterpretq_u16_u64(t10.val[0]);
    columns[5] = vreinterpretq_u16_u64(t11.val[0]);
    columns[6] = vreinterpretq_u16_u64(t10.val[1]);
    columns[7] = vreinterpretq_u16_u64(t11.val[1]);
}

static inline uint16x8_t reverse8(uint16x8_t value) {
    const uint16x8_t reversed = vrev64q_u16(value);
    return vcombine_u16(vget_high(reversed), vget_low(reversed));
}

static inline void rotate_scalar(
    uint16_t *destination,
    size_t destination_stride,
    size_t logical_width,
    size_t logical_height,
    size_t destination_x,
    size_t destination_y,
    size_t width,
    size_t height,
    const uint16_t *source,
    size_t source_stride,
    size_t source_x,
    size_t source_y,
    int clockwise
) {
    for (size_t row = 0; row < height; ++row) {
        for (size_t column = 0; column < width; ++column) {
            const size_t logical_x = destination_x + column;
            const size_t logical_y = destination_y + row;
            size_t physical_x;
            size_t physical_y;
            if (clockwise) {
                physical_x = logical_height - 1 - logical_y;
                physical_y = logical_x;
            } else {
                physical_x = logical_y;
                physical_y = logical_width - 1 - logical_x;
            }
            destination[physical_y * destination_stride + physical_x] =
                source[(source_y + row) * source_stride + source_x + column];
        }
    }
}

static void rotate_tiled(
    uint16_t *destination,
    size_t destination_stride,
    size_t logical_width,
    size_t logical_height,
    size_t destination_x,
    size_t destination_y,
    size_t width,
    size_t height,
    const uint16_t *source,
    size_t source_stride,
    size_t source_x,
    size_t source_y,
    int clockwise
) {
    for (size_t tile_y = 0; tile_y < height; tile_y += 8) {
        const size_t tile_height = (height - tile_y < 8) ? height - tile_y : 8;
        for (size_t tile_x = 0; tile_x < width; tile_x += 8) {
            const size_t tile_width = (width - tile_x < 8) ? width - tile_x : 8;
            if (tile_width != 8 || tile_height != 8) {
                rotate_scalar(
                    destination, destination_stride, logical_width, logical_height,
                    destination_x + tile_x, destination_y + tile_y, tile_width, tile_height,
                    source, source_stride, source_x + tile_x, source_y + tile_y, clockwise);
                continue;
            }

            uint16x8_t rows[8];
            uint16x8_t columns[8];
            for (size_t row = 0; row < 8; ++row) {
                rows[row] = vld1q_u16(
                    source + (source_y + tile_y + row) * source_stride + source_x + tile_x);
            }
            transpose8(rows, columns);
            for (size_t column = 0; column < 8; ++column) {
                if (clockwise) {
                    const size_t physical_x = logical_height -
                        (destination_y + tile_y + 8);
                    const size_t physical_y = destination_x + tile_x + column;
                    vst1q_u16(
                        destination + physical_y * destination_stride + physical_x,
                        reverse8(columns[column]));
                } else {
                    const size_t physical_y = logical_width - 1 -
                        (destination_x + tile_x + column);
                    const size_t physical_x = destination_y + tile_y;
                    vst1q_u16(
                        destination + physical_y * destination_stride + physical_x,
                        columns[column]);
                }
            }
        }
    }
}

void mister_magik_rgb565_rotate_clockwise(
    uint16_t *destination, size_t destination_stride,
    size_t logical_width, size_t logical_height,
    size_t destination_x, size_t destination_y, size_t width, size_t height,
    const uint16_t *source, size_t source_stride, size_t source_x, size_t source_y
) {
    rotate_tiled(destination, destination_stride, logical_width, logical_height,
                 destination_x, destination_y, width, height,
                 source, source_stride, source_x, source_y, 1);
}

void mister_magik_rgb565_rotate_counter_clockwise(
    uint16_t *destination, size_t destination_stride,
    size_t logical_width, size_t logical_height,
    size_t destination_x, size_t destination_y, size_t width, size_t height,
    const uint16_t *source, size_t source_stride, size_t source_x, size_t source_y
) {
    rotate_tiled(destination, destination_stride, logical_width, logical_height,
                 destination_x, destination_y, width, height,
                 source, source_stride, source_x, source_y, 0);
}

static inline uint16x8_t blend8(uint16x8_t from, uint16x8_t to, uint16_t alpha) {
    const uint16_t inverse = (uint16_t)(32u - alpha);
    const uint16x4_t from_lo = vget_low(from);
    const uint16x4_t from_hi = vget_high(from);
    const uint16x4_t to_lo = vget_low(to);
    const uint16x4_t to_hi = vget_high(to);
    const uint16x4_t red_blue_mask = vdup_n_u16(0xf81f);
    const uint16x4_t green_mask = vdup_n_u16(0x07e0);
    const uint16x4_t lo = vorr_u16(
        vmovn_u32(vandq_u32(vshrq_n_u32(
            vmlal_n_u16(vmull_n_u16(vand_u16(from_lo, red_blue_mask), inverse),
                        vand_u16(to_lo, red_blue_mask), alpha), 5),
            vdupq_n_u32(0xf81f))),
        vmovn_u32(vandq_u32(vshrq_n_u32(
            vmlal_n_u16(vmull_n_u16(vand_u16(from_lo, green_mask), inverse),
                        vand_u16(to_lo, green_mask), alpha), 5),
            vdupq_n_u32(0x07e0))));
    const uint16x4_t hi = vorr_u16(
        vmovn_u32(vandq_u32(vshrq_n_u32(
            vmlal_n_u16(vmull_n_u16(vand_u16(from_hi, red_blue_mask), inverse),
                        vand_u16(to_hi, red_blue_mask), alpha), 5),
            vdupq_n_u32(0xf81f))),
        vmovn_u32(vandq_u32(vshrq_n_u32(
            vmlal_n_u16(vmull_n_u16(vand_u16(from_hi, green_mask), inverse),
                        vand_u16(to_hi, green_mask), alpha), 5),
            vdupq_n_u32(0x07e0))));
    return vcombine_u16(lo, hi);
}

void mister_magik_rgb565_blend(
    uint16_t *destination, const uint16_t *previous, const uint16_t *current,
    size_t start, size_t end, uint16_t alpha
) {
    const uint16_t clamped_alpha = alpha > 32u ? 32u : alpha;
    size_t index = start;
    for (; index + 7 < end; index += 8) {
        const uint16x8_t from = vld1q_u16(previous + index);
        const uint16x8_t to = vld1q_u16(current + index);
        vst1q_u16(destination + index, blend8(from, to, clamped_alpha));
    }
    for (; index < end; ++index) {
        const uint16_t from = previous[index];
        const uint16_t to = current[index];
        const uint32_t red_blue = (
            ((uint32_t)(from & 0xf81f) * (32u - clamped_alpha)) +
            ((uint32_t)(to & 0xf81f) * clamped_alpha)) >> 5;
        const uint32_t green = (
            ((uint32_t)(from & 0x07e0) * (32u - clamped_alpha)) +
            ((uint32_t)(to & 0x07e0) * clamped_alpha)) >> 5;
        destination[index] = (uint16_t)((red_blue & 0xf81f) | (green & 0x07e0));
    }
}
