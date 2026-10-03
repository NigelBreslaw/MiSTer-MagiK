// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

static inline void transpose4(uint16x4_t rows[4], uint16x4_t columns[4]) {
    const uint16x4x2_t t0 = vtrn_u16(rows[0], rows[1]);
    const uint16x4x2_t t1 = vtrn_u16(rows[2], rows[3]);
    const uint32x2x2_t t2 = vtrn_u32(
        vreinterpret_u32_u16(t0.val[0]), vreinterpret_u32_u16(t1.val[0]));
    const uint32x2x2_t t3 = vtrn_u32(
        vreinterpret_u32_u16(t0.val[1]), vreinterpret_u32_u16(t1.val[1]));
    columns[0] = vreinterpret_u16_u32(t2.val[0]);
    columns[1] = vreinterpret_u16_u32(t3.val[0]);
    columns[2] = vreinterpret_u16_u32(t2.val[1]);
    columns[3] = vreinterpret_u16_u32(t3.val[1]);
}

static inline void transpose8(uint16x8_t rows[8], uint16x8_t columns[8]) {
    uint16x4_t top_rows[4];
    uint16x4_t bottom_rows[4];
    uint16x4_t top_columns[4];
    uint16x4_t bottom_columns[4];
    for (size_t row = 0; row < 4; ++row) {
        top_rows[row] = vget_low_u16(rows[row]);
        bottom_rows[row] = vget_low_u16(rows[row + 4]);
    }
    transpose4(top_rows, top_columns);
    transpose4(bottom_rows, bottom_columns);
    for (size_t column = 0; column < 4; ++column) {
        columns[column] = vcombine_u16(top_columns[column], bottom_columns[column]);
    }
    for (size_t row = 0; row < 4; ++row) {
        top_rows[row] = vget_high_u16(rows[row]);
        bottom_rows[row] = vget_high_u16(rows[row + 4]);
    }
    transpose4(top_rows, top_columns);
    transpose4(bottom_rows, bottom_columns);
    for (size_t column = 0; column < 4; ++column) {
        columns[column + 4] = vcombine_u16(top_columns[column], bottom_columns[column]);
    }
}

static inline uint16x8_t reverse8(uint16x8_t value) {
    const uint16x8_t reversed = vrev64q_u16(value);
    return vcombine_u16(vget_high_u16(reversed), vget_low_u16(reversed));
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
    if (alpha == 0) return from;
    if (alpha == 32) return to;
    // For 0 < alpha < 32, vqdmulh(delta, alpha << 10) is exactly
    // floor(delta * alpha / 32), including negative deltas (not truncation
    // toward zero). Unpacked channel deltas are bounded by 63, so the
    // doubled product cannot saturate. Handle alpha 32 above: its coefficient
    // is not representable in signed 16 bits. No rounding step is introduced.
    const int16x8_t coefficient = vdupq_n_s16((int16_t)(alpha << 10));
    const uint16x8_t red_from = vshrq_n_u16(from, 11);
    const uint16x8_t red_to = vshrq_n_u16(to, 11);
    const uint16x8_t green_from = vandq_u16(vshrq_n_u16(from, 5), vdupq_n_u16(63));
    const uint16x8_t green_to = vandq_u16(vshrq_n_u16(to, 5), vdupq_n_u16(63));
    const uint16x8_t blue_from = vandq_u16(from, vdupq_n_u16(31));
    const uint16x8_t blue_to = vandq_u16(to, vdupq_n_u16(31));
    const uint16x8_t red = vreinterpretq_u16_s16(vaddq_s16(
        vreinterpretq_s16_u16(red_from), vqdmulhq_s16(vsubq_s16(
            vreinterpretq_s16_u16(red_to), vreinterpretq_s16_u16(red_from)), coefficient)));
    const uint16x8_t green = vreinterpretq_u16_s16(vaddq_s16(
        vreinterpretq_s16_u16(green_from), vqdmulhq_s16(vsubq_s16(
            vreinterpretq_s16_u16(green_to), vreinterpretq_s16_u16(green_from)), coefficient)));
    const uint16x8_t blue = vreinterpretq_u16_s16(vaddq_s16(
        vreinterpretq_s16_u16(blue_from), vqdmulhq_s16(vsubq_s16(
            vreinterpretq_s16_u16(blue_to), vreinterpretq_s16_u16(blue_from)), coefficient)));
    return vorrq_u16(vorrq_u16(vshlq_n_u16(red, 11), vshlq_n_u16(green, 5)), blue);
}

static inline uint16_t blend1(uint16_t from, uint16_t to, uint16_t alpha) {
    const uint32_t red_blue = (
        ((uint32_t)(from & 0xf81f) * (32u - alpha)) +
        ((uint32_t)(to & 0xf81f) * alpha)) >> 5;
    const uint32_t green = (
        ((uint32_t)(from & 0x07e0) * (32u - alpha)) +
        ((uint32_t)(to & 0x07e0) * alpha)) >> 5;
    return (uint16_t)((red_blue & 0xf81f) | (green & 0x07e0));
}

void mister_magik_rgb565_blend(
    uint16_t *destination, const uint16_t *previous, const uint16_t *current,
    size_t start, size_t end, uint16_t alpha
) {
    const uint16_t clamped_alpha = alpha > 32u ? 32u : alpha;
    size_t index = start;
    if (clamped_alpha == 16u) {
        // Floor-average each RGB565 channel without cross-channel carries.
        // Mask each channel's low bit before shifting the differing bits.
        const uint16x8_t mask = vdupq_n_u16(0xf7de);
        for (; index + 7 < end; index += 8) {
            const uint16x8_t from = vld1q_u16(previous + index);
            const uint16x8_t to = vld1q_u16(current + index);
            const uint16x8_t half_difference =
                vshrq_n_u16(vandq_u16(veorq_u16(from, to), mask), 1);
            vst1q_u16(destination + index, vaddq_u16(vandq_u16(from, to), half_difference));
        }
        for (; index < end; ++index) {
            const uint16_t from = previous[index], to = current[index];
            destination[index] = (from & to) + (((from ^ to) & 0xf7deu) >> 1);
        }
        return;
    }
    for (; index + 7 < end; index += 8) {
        const uint16x8_t from = vld1q_u16(previous + index);
        const uint16x8_t to = vld1q_u16(current + index);
        vst1q_u16(destination + index, blend8(from, to, clamped_alpha));
    }
    for (; index < end; ++index) {
        destination[index] = blend1(previous[index], current[index], clamped_alpha);
    }
}

void mister_magik_rgb565_blend_black(
    uint16_t *destination, const uint16_t *pixels,
    size_t start, size_t end, uint16_t alpha, int fade_in
) {
    const uint16_t clamped_alpha = alpha > 32u ? 32u : alpha;
    const uint16x8_t black = vdupq_n_u16(0);
    size_t index = start;
    for (; index + 7 < end; index += 8) {
        const uint16x8_t source = vld1q_u16(pixels + index);
        const uint16x8_t from = fade_in ? black : source;
        const uint16x8_t to = fade_in ? source : black;
        vst1q_u16(destination + index, blend8(from, to, clamped_alpha));
    }
    for (; index < end; ++index) {
        const uint16_t source = pixels[index];
        destination[index] = fade_in
            ? blend1(0, source, clamped_alpha)
            : blend1(source, 0, clamped_alpha);
    }
}

// In-place spans for the opt-in live Arcade compositor. Weights and rounding
// match card_page::blend, including the second fade over transparent subjects.
void mister_magik_arcade_over(uint16_t *out,const uint16_t *source,size_t n,uint16_t alpha) {
    if(!alpha) return;
    if(alpha==32) {memcpy(out,source,n*sizeof(*out));return;}
    size_t i=0;
    for(;i+7<n;i+=8) vst1q_u16(out+i,blend8(vld1q_u16(out+i),vld1q_u16(source+i),alpha));
    for(;i<n;++i) out[i]=blend1(out[i],source[i],alpha);
}
void mister_magik_arcade_base(uint16_t *out,const uint16_t *home,const uint16_t *arcade,uint16_t a,uint16_t b,size_t y0,size_t y1) {
    const size_t cuts[5]={0,26,488,490,960};
    const uint16x8_t black=vdupq_n_u16(0);
    for(size_t y=y0;y<y1;++y) {
        if(y<77) {memcpy(out+y*960,arcade+y*960,960*sizeof(*out));continue;}
        for(size_t span=0;span<4;++span) {
            int subject=(y>=77 && y<500 && span==3) || (y>=88 && y<484 && span==1);
            size_t i=y*960+cuts[span],end=y*960+cuts[span+1];
            // Endpoint weights replace the earlier stage completely. Dispatch
            // once per span, avoiding unused Home loads and per-vector branches.
            if(b==32) {
                if(subject) memset(out+i,0,(end-i)*sizeof(*out));
                else memcpy(out+i,arcade+i,(end-i)*sizeof(*out));
            } else if(a==0) {
                if(subject || b==0) {memset(out+i,0,(end-i)*sizeof(*out));continue;}
                for(;i+7<end;i+=8) vst1q_u16(out+i,blend8(black,vld1q_u16(arcade+i),b));
                for(;i<end;++i) out[i]=blend1(0,arcade[i],b);
            } else if(b==0) {
                if(a==32) {memcpy(out+i,home+i,(end-i)*sizeof(*out));continue;}
                for(;i+7<end;i+=8) vst1q_u16(out+i,blend8(black,vld1q_u16(home+i),a));
                for(;i<end;++i) out[i]=blend1(0,home[i],a);
            } else {
                for(;i+7<end;i+=8) {
                    uint16x8_t base=blend8(black,vld1q_u16(home+i),a);
                    uint16x8_t chrome=subject?black:vld1q_u16(arcade+i);
                    vst1q_u16(out+i,blend8(base,chrome,b));
                }
                for(;i<end;++i) out[i]=blend1(blend1(0,home[i],a),subject?0:arcade[i],b);
            }
        }
    }
}

// Exact bit replication, matching the portable RGB565-to-RGB8 expansion.
void mister_magik_rgb565_expand_rgb8(uint8_t *out, const uint16_t *source, size_t n) {
    size_t i = 0;
    for (; i + 7 < n; i += 8) {
        const uint16x8_t pixels = vld1q_u16(source + i);
        const uint8x8_t r = vmovn_u16(vshrq_n_u16(pixels, 11));
        const uint8x8_t g = vand_u8(vmovn_u16(vshrq_n_u16(pixels, 5)), vdup_n_u8(63));
        const uint8x8_t b = vand_u8(vmovn_u16(pixels), vdup_n_u8(31));
        uint8x8x3_t rgb;
        rgb.val[0] = vorr_u8(vshl_n_u8(r, 3), vshr_n_u8(r, 2));
        rgb.val[1] = vorr_u8(vshl_n_u8(g, 2), vshr_n_u8(g, 4));
        rgb.val[2] = vorr_u8(vshl_n_u8(b, 3), vshr_n_u8(b, 2));
        vst3_u8(out + i * 3, rgb);
    }
    for (; i < n; ++i) {
        const uint16_t pixel = source[i];
        const uint16_t r = pixel >> 11, g = (pixel >> 5) & 63, b = pixel & 31;
        out[i * 3] = (r << 3) | (r >> 2);
        out[i * 3 + 1] = (g << 2) | (g >> 4);
        out[i * 3 + 2] = (b << 3) | (b >> 2);
    }
}
