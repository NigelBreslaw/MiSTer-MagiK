// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

#define ORIENTATION_COLUMNS 16u
#define ORIENTATION_ROWS 9u
#define ORIENTATION_LEVELS 32u
#define ORIENTATION_SKIP 255u

static inline uint16x8_t dim_rgb565(uint16x8_t packed, uint16_t opacity) {
    const uint16x8_t red_blue_mask = vdupq_n_u16(0x001f);
    const uint16x8_t green_mask = vdupq_n_u16(0x003f);
    const uint16x8_t opacity_vector = vdupq_n_u16(opacity);
    uint16x8_t red = vshrq_n_u16(packed, 11);
    uint16x8_t green = vandq_u16(vshrq_n_u16(packed, 5), green_mask);
    uint16x8_t blue = vandq_u16(packed, red_blue_mask);
    red = vshrq_n_u16(vmulq_u16(red, opacity_vector), 5);
    green = vshrq_n_u16(vmulq_u16(green, opacity_vector), 5);
    blue = vshrq_n_u16(vmulq_u16(blue, opacity_vector), 5);
    return vorrq_u16(
        vshlq_n_u16(red, 11),
        vorrq_u16(vshlq_n_u16(green, 5), blue)
    );
}

static inline void copy_pixels(
    const uint16_t *restrict source,
    uint16_t *restrict output,
    size_t count
) {
    size_t index = 0;
    for (; index + 32 <= count; index += 32) {
        __builtin_prefetch(source + index + 64, 0, 1);
        const uint16x8_t pixels0 = vld1q_u16(source + index);
        const uint16x8_t pixels1 = vld1q_u16(source + index + 8);
        const uint16x8_t pixels2 = vld1q_u16(source + index + 16);
        const uint16x8_t pixels3 = vld1q_u16(source + index + 24);
        vst1q_u16(output + index, pixels0);
        vst1q_u16(output + index + 8, pixels1);
        vst1q_u16(output + index + 16, pixels2);
        vst1q_u16(output + index + 24, pixels3);
    }
    for (; index + 8 <= count; index += 8) {
        vst1q_u16(output + index, vld1q_u16(source + index));
    }
    for (; index < count; index++) {
        output[index] = source[index];
    }
}

static inline void zero_pixels(uint16_t *output, size_t count) {
    const uint16x8_t zero = vdupq_n_u16(0);
    size_t index = 0;
    for (; index + 32 <= count; index += 32) {
        vst1q_u16(output + index, zero);
        vst1q_u16(output + index + 8, zero);
        vst1q_u16(output + index + 16, zero);
        vst1q_u16(output + index + 24, zero);
    }
    for (; index + 8 <= count; index += 8) {
        vst1q_u16(output + index, zero);
    }
    for (; index < count; index++) {
        output[index] = 0;
    }
}

static inline void dim_pixels(
    const uint16_t *restrict source,
    uint16_t *restrict output,
    size_t count,
    uint16_t opacity
) {
    size_t index = 0;
    for (; index + 32 <= count; index += 32) {
        __builtin_prefetch(source + index + 64, 0, 1);
        vst1q_u16(output + index, dim_rgb565(vld1q_u16(source + index), opacity));
        vst1q_u16(output + index + 8, dim_rgb565(vld1q_u16(source + index + 8), opacity));
        vst1q_u16(output + index + 16, dim_rgb565(vld1q_u16(source + index + 16), opacity));
        vst1q_u16(output + index + 24, dim_rgb565(vld1q_u16(source + index + 24), opacity));
    }
    for (; index + 8 <= count; index += 8) {
        vst1q_u16(output + index, dim_rgb565(vld1q_u16(source + index), opacity));
    }
    for (; index < count; index++) {
        const uint32_t pixel = source[index];
        const uint32_t red_blue = (((pixel & 0xf81fu) * opacity) >> 5) & 0xf81fu;
        const uint32_t green = (((pixel & 0x07e0u) * opacity) >> 5) & 0x07e0u;
        output[index] = (uint16_t)(red_blue | green);
    }
}

__attribute__((noinline))
void mister_magik_orientation_fade_neon(
    const uint16_t *restrict source,
    uint16_t *restrict output,
    size_t width,
    size_t height,
    const uint8_t *restrict levels,
    const uint16_t *restrict dirty_rows
) {
    for (size_t tile_row = 0; tile_row < ORIENTATION_ROWS; tile_row++) {
        const size_t y0 = tile_row * height / ORIENTATION_ROWS;
        const size_t y1 = (tile_row + 1) * height / ORIENTATION_ROWS;
        for (size_t y = y0; y < y1; y++) {
            const size_t row = y * width;
            for (size_t tile_column = 0; tile_column < ORIENTATION_COLUMNS; tile_column++) {
                if ((dirty_rows[tile_row] & (1u << tile_column)) == 0) {
                    continue;
                }
                const size_t x0 = tile_column * width / ORIENTATION_COLUMNS;
                const size_t x1 = (tile_column + 1) * width / ORIENTATION_COLUMNS;
                const size_t count = x1 - x0;
                const uint16_t opacity = levels[tile_row * ORIENTATION_COLUMNS + tile_column];
                if (opacity == 0) {
                    zero_pixels(output + row + x0, count);
                } else if (opacity >= ORIENTATION_LEVELS) {
                    copy_pixels(source + row + x0, output + row + x0, count);
                } else {
                    dim_pixels(source + row + x0, output + row + x0, count, opacity);
                }
            }
        }
    }
}

static inline void centered_span(
    size_t start,
    size_t end,
    uint8_t level,
    size_t *centered_start,
    size_t *centered_end
) {
    const size_t span = end - start;
    const size_t scaled = ((span - 1) * level + ORIENTATION_LEVELS / 2) / ORIENTATION_LEVELS;
    const size_t visible = 1 + scaled < span ? 1 + scaled : span;
    const size_t center = start + (span - 1) / 2;
    const size_t half = (visible - 1) / 2;
    const size_t first = center >= half ? center - half : start;
    *centered_start = first > start ? first : start;
    const size_t after = *centered_start + visible;
    *centered_end = after < end ? after : end;
}

__attribute__((noinline))
void mister_magik_orientation_zoom_neon(
    const uint16_t *restrict source,
    uint16_t *restrict output,
    size_t width,
    size_t height,
    const uint8_t *restrict black_levels,
    const uint16_t *restrict dirty_rows
) {
    for (size_t tile_row = 0; tile_row < ORIENTATION_ROWS; tile_row++) {
        const size_t tile_y0 = tile_row * height / ORIENTATION_ROWS;
        const size_t tile_y1 = (tile_row + 1) * height / ORIENTATION_ROWS;
        size_t active_tile_x0[ORIENTATION_COLUMNS];
        size_t active_tile_x1[ORIENTATION_COLUMNS];
        size_t active_black_x0[ORIENTATION_COLUMNS];
        size_t active_black_x1[ORIENTATION_COLUMNS];
        size_t active_black_y0[ORIENTATION_COLUMNS];
        size_t active_black_y1[ORIENTATION_COLUMNS];
        uint8_t active_skip[ORIENTATION_COLUMNS];
        size_t active_columns = 0;
        for (size_t tile_column = 0; tile_column < ORIENTATION_COLUMNS; tile_column++) {
            if ((dirty_rows[tile_row] & (1u << tile_column)) == 0) {
                continue;
            }
            const size_t tile_x0 = tile_column * width / ORIENTATION_COLUMNS;
            const size_t tile_x1 = (tile_column + 1) * width / ORIENTATION_COLUMNS;
            const uint8_t level = black_levels[tile_row * ORIENTATION_COLUMNS + tile_column];
            active_tile_x0[active_columns] = tile_x0;
            active_tile_x1[active_columns] = tile_x1;
            active_skip[active_columns] = level == ORIENTATION_SKIP;
            if (level != ORIENTATION_SKIP) {
                centered_span(
                    tile_x0,
                    tile_x1,
                    level,
                    &active_black_x0[active_columns],
                    &active_black_x1[active_columns]
                );
                centered_span(
                    tile_y0,
                    tile_y1,
                    level,
                    &active_black_y0[active_columns],
                    &active_black_y1[active_columns]
                );
            }
            active_columns++;
        }
        for (size_t y = tile_y0; y < tile_y1; y++) {
            const size_t row = y * width;
            for (size_t active = 0; active < active_columns; active++) {
                const size_t tile_x0 = active_tile_x0[active];
                const size_t tile_x1 = active_tile_x1[active];
                if (active_skip[active]
                    || y < active_black_y0[active]
                    || y >= active_black_y1[active]) {
                    copy_pixels(
                        source + row + tile_x0,
                        output + row + tile_x0,
                        tile_x1 - tile_x0
                    );
                    continue;
                }
                const size_t x0 = active_black_x0[active];
                const size_t x1 = active_black_x1[active];
                copy_pixels(source + row + tile_x0, output + row + tile_x0, x0 - tile_x0);
                zero_pixels(output + row + x0, x1 - x0);
                copy_pixels(source + row + x1, output + row + x1, tile_x1 - x1);
            }
        }
    }
}

typedef struct { size_t x0, y0, x1, y1; } zoom_rect;

static zoom_rect black_rect(size_t width, size_t height, size_t tile, uint8_t level) {
    zoom_rect r = {0,0,0,0};
    if (level == ORIENTATION_SKIP) return r;
    const size_t row = tile / ORIENTATION_COLUMNS, col = tile % ORIENTATION_COLUMNS;
    centered_span(col * width / ORIENTATION_COLUMNS, (col+1) * width / ORIENTATION_COLUMNS,
        level, &r.x0, &r.x1);
    centered_span(row * height / ORIENTATION_ROWS, (row+1) * height / ORIENTATION_ROWS,
        level, &r.y0, &r.y1);
    return r;
}

static size_t paint_rect(const uint16_t *source, uint16_t *output,
    size_t width, zoom_rect r, int black) {
    for (size_t y = r.y0; y < r.y1; ++y) {
        if (black) zero_pixels(output + y * width + r.x0, r.x1-r.x0);
        else copy_pixels(source + y * width + r.x0, output + y * width + r.x0, r.x1-r.x0);
    }
    return (r.x1-r.x0) * (r.y1-r.y0);
}

static size_t paint_difference(const uint16_t *source, uint16_t *output,
    size_t width, zoom_rect a, zoom_rect b, int black) {
    const size_t x0 = a.x0 > b.x0 ? a.x0 : b.x0;
    const size_t y0 = a.y0 > b.y0 ? a.y0 : b.y0;
    const size_t x1 = a.x1 < b.x1 ? a.x1 : b.x1;
    const size_t y1 = a.y1 < b.y1 ? a.y1 : b.y1;
    if (x0 >= x1 || y0 >= y1) return paint_rect(source, output, width, a, black);
    size_t writes = paint_rect(source, output, width, (zoom_rect){a.x0,a.y0,a.x1,y0},black);
    writes += paint_rect(source, output, width, (zoom_rect){a.x0,y1,a.x1,a.y1},black);
    writes += paint_rect(source, output, width, (zoom_rect){a.x0,y0,x0,y1},black);
    writes += paint_rect(source, output, width, (zoom_rect){x1,y0,a.x1,y1},black);
    return writes;
}

size_t mister_magik_orientation_zoom_delta_neon(const uint16_t *restrict source,
    uint16_t *restrict output, size_t width, size_t height,
    const uint8_t *restrict levels, const uint8_t *restrict previous,
    const uint16_t *restrict dirty_rows) {
    size_t writes = 0;
    for (size_t row = 0; row < ORIENTATION_ROWS; ++row) {
        for (size_t col = 0; col < ORIENTATION_COLUMNS; ++col) {
            if ((dirty_rows[row] & (1u << col)) == 0) continue;
            const size_t tile = row * ORIENTATION_COLUMNS + col;
            const zoom_rect old = black_rect(width,height,tile,previous[tile]);
            const zoom_rect now = black_rect(width,height,tile,levels[tile]);
            writes += paint_difference(source,output,width,old,now,0);
            writes += paint_difference(source,output,width,now,old,1);
        }
    }
    return writes;
}
