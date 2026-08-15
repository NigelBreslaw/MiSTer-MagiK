// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_crt_backdrop_blend_neon(const uint16_t *from,
                                          const uint16_t *to,
                                          uint16_t *destination,
                                          size_t length,
                                          uint32_t alpha_bucket) {
    const uint32_t alpha = alpha_bucket > 32u ? 32u : alpha_bucket;
    const uint32_t inverse = 32u - alpha;
    size_t index = 0;
    const uint32x4_t red_blue_mask = vdupq_n_u32(0xf81fu);
    const uint32x4_t green_mask = vdupq_n_u32(0x07e0u);

    for (; index + 4u <= length; index += 4u) {
        const uint16x4_t from16 = vld1_u16(from + index);
        const uint16x4_t to16 = vld1_u16(to + index);
        const uint32x4_t from32 = vmovl_u16(from16);
        const uint32x4_t to32 = vmovl_u16(to16);
        const uint32x4_t red_blue =
            vshrq_n_u32(vaddq_u32(vmulq_n_u32(vandq_u32(from32, red_blue_mask), inverse),
                                  vmulq_n_u32(vandq_u32(to32, red_blue_mask), alpha)),
                        5);
        const uint32x4_t green =
            vshrq_n_u32(vaddq_u32(vmulq_n_u32(vandq_u32(from32, green_mask), inverse),
                                  vmulq_n_u32(vandq_u32(to32, green_mask), alpha)),
                        5);
        const uint16x4_t output = vmovn_u32(vorrq_u32(vandq_u32(red_blue, red_blue_mask),
                                                       vandq_u32(green, green_mask)));
        vst1_u16(destination + index, output);
    }

    for (; index < length; ++index) {
        const uint32_t from_pixel = from[index];
        const uint32_t to_pixel = to[index];
        const uint32_t red_blue =
            (((from_pixel & 0xf81fu) * inverse + (to_pixel & 0xf81fu) * alpha) >> 5) & 0xf81fu;
        const uint32_t green =
            (((from_pixel & 0x07e0u) * inverse + (to_pixel & 0x07e0u) * alpha) >> 5) & 0x07e0u;
        destination[index] = (uint16_t)(red_blue | green);
    }
}
