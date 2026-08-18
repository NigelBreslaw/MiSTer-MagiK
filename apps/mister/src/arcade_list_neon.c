// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void mister_magik_arcade_selection_rgb565(
    uint16_t *restrict destination,
    const uint16_t *restrict source,
    size_t count,
    uint16_t background,
    uint16_t alternate_background,
    uint16_t border,
    uint16_t badge_fill,
    uint16_t selection_fill,
    uint16_t selection_foreground,
    uint8_t fixed_foreground
) {
    const uint16x8_t background_v = vdupq_n_u16(background);
    const uint16x8_t alternate_background_v = vdupq_n_u16(alternate_background);
    const uint16x8_t border_v = vdupq_n_u16(border);
    const uint16x8_t badge_fill_v = vdupq_n_u16(badge_fill);
    const uint16x8_t selection_fill_v = vdupq_n_u16(selection_fill);
    const uint16x8_t selection_foreground_v = vdupq_n_u16(selection_foreground);
    size_t index = 0;

    for (; index + 7 < count; index += 8) {
        const uint16x8_t pixels = vld1q_u16(source + index);
        uint16x8_t background_mask = vceqq_u16(pixels, background_v);
        background_mask = vorrq_u16(
            background_mask,
            vceqq_u16(pixels, alternate_background_v)
        );
        background_mask = vorrq_u16(background_mask, vceqq_u16(pixels, border_v));
        background_mask = vorrq_u16(background_mask, vceqq_u16(pixels, badge_fill_v));
        const uint16x8_t foreground = fixed_foreground
            ? selection_foreground_v
            : vmvnq_u16(pixels);
        vst1q_u16(
            destination + index,
            vbslq_u16(background_mask, selection_fill_v, foreground)
        );
    }

    for (; index < count; ++index) {
        const uint16_t pixel = source[index];
        const uint8_t is_background =
            pixel == background ||
            pixel == alternate_background ||
            pixel == border ||
            pixel == badge_fill;
        destination[index] = is_background
            ? selection_fill
            : (fixed_foreground ? selection_foreground : (uint16_t)~pixel);
    }
}
