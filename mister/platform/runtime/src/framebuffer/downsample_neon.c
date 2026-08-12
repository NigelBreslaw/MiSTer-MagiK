// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_downsample_rgb565_2x_neon(const uint16_t *source,
                                             size_t source_width,
                                             size_t source_height,
                                             size_t source_stride,
                                             uint16_t *destination,
                                             size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;

    for (size_t output_y = 0; output_y < destination_height; ++output_y) {
        const uint16_t *source_row = source + output_y * 2U * source_stride;
        uint16_t *destination_row = destination + output_y * destination_width;
        size_t output_x = 0;
        size_t source_x = 0;
        for (; output_x + 8U <= destination_width && source_x + 16U <= source_width;
             output_x += 8U, source_x += 16U) {
            const uint16x8x2_t even_odd = vld2q_u16(source_row + source_x);
            vst1q_u16(destination_row + output_x, even_odd.val[0]);
        }
        for (; output_x < destination_width; ++output_x, source_x += 2U) {
            destination_row[output_x] = source_row[source_x];
        }
    }
}
