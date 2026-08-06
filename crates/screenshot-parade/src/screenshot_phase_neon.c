// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

#include "screenshot_phase_reciprocals.h"

static inline uint16x4_t reconstruct_six_tap(
    uint16x4_t value0,
    uint16x4_t value1,
    uint16x4_t value2,
    uint16x4_t value3,
    uint16x4_t value4,
    uint16x4_t value5,
    const int32_t *restrict weights
) {
    uint16x4_t minima = vmin_u16(value0, value1);
    uint16x4_t maxima = vmax_u16(value0, value1);
    minima = vmin_u16(minima, value2);
    maxima = vmax_u16(maxima, value2);
    minima = vmin_u16(minima, value3);
    maxima = vmax_u16(maxima, value3);
    minima = vmin_u16(minima, value4);
    maxima = vmax_u16(maxima, value4);
    minima = vmin_u16(minima, value5);
    maxima = vmax_u16(maxima, value5);

    int32x4_t sums = vmulq_n_s32(
        vreinterpretq_s32_u32(vmovl_u16(value0)),
        weights[0]
    );
    sums = vmlaq_n_s32(sums, vreinterpretq_s32_u32(vmovl_u16(value1)), weights[1]);
    sums = vmlaq_n_s32(sums, vreinterpretq_s32_u32(vmovl_u16(value2)), weights[2]);
    sums = vmlaq_n_s32(sums, vreinterpretq_s32_u32(vmovl_u16(value3)), weights[3]);
    sums = vmlaq_n_s32(sums, vreinterpretq_s32_u32(vmovl_u16(value4)), weights[4]);
    sums = vmlaq_n_s32(sums, vreinterpretq_s32_u32(vmovl_u16(value5)), weights[5]);

    int32x4_t reconstructed = vshrq_n_s32(
        vaddq_s32(sums, vdupq_n_s32(1 << 13)),
        14
    );
    reconstructed = vmaxq_s32(
        reconstructed,
        vreinterpretq_s32_u32(vmovl_u16(minima))
    );
    reconstructed = vminq_s32(
        reconstructed,
        vreinterpretq_s32_u32(vmovl_u16(maxima))
    );
    return vmovn_u32(vreinterpretq_u32_s32(reconstructed));
}

static inline uint16_t unpremultiply_exact(
    uint16_t channel,
    uint16_t alpha
) {
    if (alpha == UINT16_MAX) {
        return channel;
    }
    const uint32_t numerator = (uint32_t)channel * UINT16_MAX + alpha / 2u;
    uint32_t quotient;
    if (alpha == 1) {
        quotient = numerator;
    } else {
        quotient = (uint32_t)(
            ((uint64_t)numerator * SCREENSHOT_PHASE_RECIPROCALS[alpha]) >> 32
        );
        const uint32_t remainder = numerator - quotient * alpha;
        quotient += remainder >= alpha;
    }
    return quotient < UINT16_MAX ? (uint16_t)quotient : UINT16_MAX;
}

static inline uint8_t linear_to_srgb8(
    uint16_t value,
    const uint8_t *restrict linear_to_srgb
) {
    uint32_t rounded = (uint32_t)value + 8u;
    if (rounded > UINT16_MAX) {
        rounded = UINT16_MAX;
    }
    return linear_to_srgb[rounded >> 4];
}

static inline void write_phase_pixel(
    uint16x4_t reconstructed,
    const uint8_t *restrict linear_to_srgb,
    uint16_t *restrict pixel,
    uint8_t *restrict coverage
) {
    const uint16_t alpha = vget_lane_u16(reconstructed, 3);
    *coverage = (uint8_t)(((uint32_t)alpha + 128u) / 257u);
    if (alpha == 0) {
        *pixel = 0;
        return;
    }
    const uint8_t r = linear_to_srgb8(
        unpremultiply_exact(vget_lane_u16(reconstructed, 0), alpha),
        linear_to_srgb
    );
    const uint8_t g = linear_to_srgb8(
        unpremultiply_exact(vget_lane_u16(reconstructed, 1), alpha),
        linear_to_srgb
    );
    const uint8_t b = linear_to_srgb8(
        unpremultiply_exact(vget_lane_u16(reconstructed, 2), alpha),
        linear_to_srgb
    );
    *pixel = (uint16_t)(((uint16_t)(r >> 3) << 11) |
        ((uint16_t)(g >> 2) << 5) | (uint16_t)(b >> 3));
}

static inline void write_opaque_phase_pixel(
    uint16x4_t reconstructed,
    const uint8_t *restrict linear_to_srgb,
    uint16_t *restrict pixel,
    uint8_t *restrict coverage
) {
    const uint8_t r = linear_to_srgb8(
        vget_lane_u16(reconstructed, 0), linear_to_srgb
    );
    const uint8_t g = linear_to_srgb8(
        vget_lane_u16(reconstructed, 1), linear_to_srgb
    );
    const uint8_t b = linear_to_srgb8(
        vget_lane_u16(reconstructed, 2), linear_to_srgb
    );
    *pixel = (uint16_t)(((uint16_t)(r >> 3) << 11) |
        ((uint16_t)(g >> 2) << 5) | (uint16_t)(b >> 3));
    *coverage = UINT8_MAX;
}

static inline uint16x4_t reconstruct_interior_pixel(
    const uint16_t *restrict input,
    const int32_t *restrict weights
) {
    return reconstruct_six_tap(
        vld1_u16(input),
        vld1_u16(input + 4),
        vld1_u16(input + 8),
        vld1_u16(input + 12),
        vld1_u16(input + 16),
        vld1_u16(input + 20),
        weights
    );
}

static inline uint16x4_t load_or_zero(
    const uint16_t *restrict row_source,
    size_t source_width,
    ptrdiff_t source_x,
    uint16x4_t zero
) {
    return source_x >= 0 && (size_t)source_x < source_width
        ? vld1_u16(row_source + (size_t)source_x * 4)
        : zero;
}

static inline void reconstruct_edge_pixel(
    const uint16_t *restrict row_source,
    size_t source_width,
    size_t output_x,
    const int32_t *restrict weights,
    const uint8_t *restrict linear_to_srgb,
    uint16_t *restrict pixel,
    uint8_t *restrict coverage
) {
    const uint16x4_t zero = vdup_n_u16(0);
    const ptrdiff_t source_x = (ptrdiff_t)output_x - 3;
    write_phase_pixel(
        reconstruct_six_tap(
            load_or_zero(row_source, source_width, source_x, zero),
            load_or_zero(row_source, source_width, source_x + 1, zero),
            load_or_zero(row_source, source_width, source_x + 2, zero),
            load_or_zero(row_source, source_width, source_x + 3, zero),
            load_or_zero(row_source, source_width, source_x + 4, zero),
            load_or_zero(row_source, source_width, source_x + 5, zero),
            weights
        ),
        linear_to_srgb,
        pixel,
        coverage
    );
}

void mister_magik_screenshot_phase_neon(
    const uint16_t *restrict source,
    size_t source_width,
    size_t height,
    size_t output_width,
    const int32_t *restrict weights,
    const uint16_t *restrict source_opaque_spans,
    const uint8_t *restrict linear_to_srgb,
    uint16_t *restrict pixels,
    uint8_t *restrict coverage
) {
    const size_t interior_start = source_width >= 6 ? 3 : output_width;
    const size_t interior_end = source_width >= 6 ? source_width - 2 : output_width;

    for (size_t y = 0; y < height; ++y) {
        const uint16_t *row_source = source + y * source_width * 4;
        uint16_t *row_pixels = pixels + y * output_width;
        uint8_t *row_coverage = coverage + y * output_width;
        const size_t source_opaque_start = source_opaque_spans[y * 2];
        const size_t source_opaque_end = source_opaque_spans[y * 2 + 1];
        size_t opaque_start = source_opaque_start + 3;
        size_t opaque_end = source_opaque_end > 2 ? source_opaque_end - 2 : 0;
        if (opaque_start < interior_start) {
            opaque_start = interior_start;
        }
        if (opaque_start > interior_end) {
            opaque_start = interior_end;
        }
        if (opaque_end < opaque_start) {
            opaque_end = opaque_start;
        }
        if (opaque_end > interior_end) {
            opaque_end = interior_end;
        }
        size_t output_x = 0;

        for (; output_x < interior_start; ++output_x) {
            reconstruct_edge_pixel(
                row_source,
                source_width,
                output_x,
                weights,
                linear_to_srgb,
                row_pixels + output_x,
                row_coverage + output_x
            );
        }
        for (; output_x < opaque_start; ++output_x) {
            const uint16_t *input = row_source + (output_x - 3) * 4;
            write_phase_pixel(
                reconstruct_interior_pixel(input, weights),
                linear_to_srgb,
                row_pixels + output_x,
                row_coverage + output_x
            );
        }
        for (; output_x + 1 < opaque_end; output_x += 2) {
            const uint16_t *input = row_source + (output_x - 3) * 4;
            const uint16x4_t value0 = vld1_u16(input);
            const uint16x4_t value1 = vld1_u16(input + 4);
            const uint16x4_t value2 = vld1_u16(input + 8);
            const uint16x4_t value3 = vld1_u16(input + 12);
            const uint16x4_t value4 = vld1_u16(input + 16);
            const uint16x4_t value5 = vld1_u16(input + 20);
            const uint16x4_t value6 = vld1_u16(input + 24);
            write_opaque_phase_pixel(
                reconstruct_six_tap(
                    value0, value1, value2, value3, value4, value5, weights
                ),
                linear_to_srgb,
                row_pixels + output_x,
                row_coverage + output_x
            );
            write_opaque_phase_pixel(
                reconstruct_six_tap(
                    value1, value2, value3, value4, value5, value6, weights
                ),
                linear_to_srgb,
                row_pixels + output_x + 1,
                row_coverage + output_x + 1
            );
        }
        if (output_x < opaque_end) {
            const uint16_t *input = row_source + (output_x - 3) * 4;
            write_opaque_phase_pixel(
                reconstruct_interior_pixel(input, weights),
                linear_to_srgb,
                row_pixels + output_x,
                row_coverage + output_x
            );
            ++output_x;
        }
        for (; output_x < interior_end; ++output_x) {
            const uint16_t *input = row_source + (output_x - 3) * 4;
            write_phase_pixel(
                reconstruct_interior_pixel(input, weights),
                linear_to_srgb,
                row_pixels + output_x,
                row_coverage + output_x
            );
        }
        for (; output_x < output_width; ++output_x) {
            reconstruct_edge_pixel(
                row_source,
                source_width,
                output_x,
                weights,
                linear_to_srgb,
                row_pixels + output_x,
                row_coverage + output_x
            );
        }
    }
}

typedef struct {
    uint32_t sample_start;
    uint16_t sample_count;
    uint16_t padding;
} mister_magik_polyphase_filter_command;

void mister_magik_screenshot_direct_phase_neon(
    const uint16_t *restrict source,
    size_t source_width,
    size_t height,
    const mister_magik_polyphase_filter_command *restrict commands,
    size_t output_width,
    const uint16_t *restrict sample_indices,
    const int16_t *restrict weights,
    const uint8_t *restrict linear_to_srgb,
    uint16_t *restrict pixels
) {
    const int32x4_t rounding = vdupq_n_s32(1 << 13);
    const int32x4_t minimum = vdupq_n_s32(0);
    const int32x4_t maximum = vdupq_n_s32(UINT16_MAX);
    for (size_t y = 0; y < height; ++y) {
        const uint16_t *row_source = source + y * source_width * 4;
        uint16_t *row_pixels = pixels + y * output_width;
        for (size_t output_x = 0; output_x < output_width; ++output_x) {
            const mister_magik_polyphase_filter_command command = commands[output_x];
            int32x4_t sums = vdupq_n_s32(0);
            const size_t end = command.sample_start + command.sample_count;
            for (size_t tap = command.sample_start; tap < end; ++tap) {
                const uint16x4_t sample =
                    vld1_u16(row_source + (size_t)sample_indices[tap] * 4);
                sums = vmlaq_n_s32(
                    sums,
                    vreinterpretq_s32_u32(vmovl_u16(sample)),
                    weights[tap]
                );
            }
            int32x4_t reconstructed = vshrq_n_s32(vaddq_s32(sums, rounding), 14);
            reconstructed = vmaxq_s32(reconstructed, minimum);
            reconstructed = vminq_s32(reconstructed, maximum);
            const uint16x4_t linear = vmovn_u32(vreinterpretq_u32_s32(reconstructed));
            const uint8_t r = linear_to_srgb8(vget_lane_u16(linear, 0), linear_to_srgb);
            const uint8_t g = linear_to_srgb8(vget_lane_u16(linear, 1), linear_to_srgb);
            const uint8_t b = linear_to_srgb8(vget_lane_u16(linear, 2), linear_to_srgb);
            row_pixels[output_x] = (uint16_t)(((uint16_t)(r >> 3) << 11) |
                ((uint16_t)(g >> 2) << 5) | (uint16_t)(b >> 3));
        }
    }
}
