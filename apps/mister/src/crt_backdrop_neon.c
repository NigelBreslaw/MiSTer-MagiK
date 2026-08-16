// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

static inline uint16x4_t blend_rgb565_four(
    uint16x4_t from,
    uint16x4_t to,
    uint16_t alpha,
    uint16_t inverse
) {
    const uint16x4_t red_blue_mask = vdup_n_u16(0xf81f);
    const uint16x4_t green_mask = vdup_n_u16(0x07e0);
    const uint32x4_t red_blue = vmlal_n_u16(
        vmull_n_u16(vand_u16(from, red_blue_mask), inverse),
        vand_u16(to, red_blue_mask),
        alpha
    );
    const uint32x4_t green = vmlal_n_u16(
        vmull_n_u16(vand_u16(from, green_mask), inverse),
        vand_u16(to, green_mask),
        alpha
    );
    const uint16x4_t red_blue_out = vmovn_u32(
        vandq_u32(vshrq_n_u32(red_blue, 5), vdupq_n_u32(0xf81f))
    );
    const uint16x4_t green_out = vmovn_u32(
        vandq_u32(vshrq_n_u32(green, 5), vdupq_n_u32(0x07e0))
    );
    return vorr_u16(red_blue_out, green_out);
}

void mister_magik_crt_backdrop_blend_coarse_two(
    uint16_t *restrict destination,
    const uint16_t *restrict previous,
    const uint16_t *restrict current,
    size_t start,
    size_t end,
    uint16_t alpha
) {
    const uint16_t inverse = (uint16_t)(32u - (alpha > 32u ? 32u : alpha));
    const uint16_t clamped_alpha = alpha > 32u ? 32u : alpha;
    size_t index = start;

    for (; index + 7 < end; index += 8) {
        // The coarse compositor samples every other source pixel and expands
        // each result to a two-pixel horizontal block.
        const uint16x4x2_t from_even_odd = vld2_u16(previous + index);
        const uint16x4x2_t to_even_odd = vld2_u16(current + index);
        const uint16x4_t blended = blend_rgb565_four(
            from_even_odd.val[0],
            to_even_odd.val[0],
            clamped_alpha,
            inverse
        );
        const uint16x4x2_t expanded = vzip_u16(blended, blended);
        vst1_u16(destination + index, expanded.val[0]);
        vst1_u16(destination + index + 4, expanded.val[1]);
    }

    for (; index + 1 < end; index += 2) {
        const uint16_t from = previous[index];
        const uint16_t to = current[index];
        const uint16_t red_blue = (uint16_t)((
            (((uint32_t)(from & 0xf81f) * inverse) +
             ((uint32_t)(to & 0xf81f) * clamped_alpha)) >> 5
        ) & 0xf81f);
        const uint16_t green = (uint16_t)((
            (((uint32_t)(from & 0x07e0) * inverse) +
             ((uint32_t)(to & 0x07e0) * clamped_alpha)) >> 5
        ) & 0x07e0);
        const uint16_t pixel = (uint16_t)(red_blue | green);
        destination[index] = pixel;
        destination[index + 1] = pixel;
    }
    if (index < end) {
        const uint16_t from = previous[index];
        const uint16_t to = current[index];
        const uint16_t red_blue = (uint16_t)((
            (((uint32_t)(from & 0xf81f) * inverse) +
             ((uint32_t)(to & 0xf81f) * clamped_alpha)) >> 5
        ) & 0xf81f);
        const uint16_t green = (uint16_t)((
            (((uint32_t)(from & 0x07e0) * inverse) +
             ((uint32_t)(to & 0x07e0) * clamped_alpha)) >> 5
        ) & 0x07e0);
        destination[index] = (uint16_t)(red_blue | green);
    }
}
