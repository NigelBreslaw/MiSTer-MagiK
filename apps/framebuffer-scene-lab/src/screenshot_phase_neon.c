// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_screenshot_phase_neon(
    const uint16_t *restrict source,
    size_t source_width,
    size_t height,
    size_t output_width,
    const int32_t *restrict weights,
    uint16_t *restrict output
) {
    const uint16x4_t zero_u16 = vdup_n_u16(0);
    const int32x4_t rounding = vdupq_n_s32(1 << 13);

    for (size_t y = 0; y < height; ++y) {
        for (size_t out_x = 0; out_x < output_width; ++out_x) {
            int32x4_t sums = vdupq_n_s32(0);
            uint16x4_t minima = vdup_n_u16(UINT16_MAX);
            uint16x4_t maxima = zero_u16;

            for (size_t tap = 0; tap < 6; ++tap) {
                const ptrdiff_t source_x = (ptrdiff_t)out_x + (ptrdiff_t)tap - 3;
                const uint16x4_t values =
                    source_x >= 0 && (size_t)source_x < source_width
                        ? vld1_u16(source + (y * source_width + (size_t)source_x) * 4)
                        : zero_u16;
                minima = vmin_u16(minima, values);
                maxima = vmax_u16(maxima, values);
                sums = vmlaq_n_s32(
                    sums,
                    vreinterpretq_s32_u32(vmovl_u16(values)),
                    weights[tap]
                );
            }

            int32x4_t reconstructed = vshrq_n_s32(vaddq_s32(sums, rounding), 14);
            const int32x4_t minimum = vreinterpretq_s32_u32(vmovl_u16(minima));
            const int32x4_t maximum = vreinterpretq_s32_u32(vmovl_u16(maxima));
            reconstructed = vmaxq_s32(reconstructed, minimum);
            reconstructed = vminq_s32(reconstructed, maximum);
            vst1_u16(
                output + (y * output_width + out_x) * 4,
                vmovn_u32(vreinterpretq_u32_s32(reconstructed))
            );
        }
    }
}
