// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

void magik_launcher_copy_row(uint16_t *out, const uint16_t *src, size_t n) {
  size_t i = 0;
  for (; i + 16 <= n; i += 16) {
    uint16x8_t a = vld1q_u16(src + i);
    uint16x8_t b = vld1q_u16(src + i + 8);
    vst1q_u16(out + i, a);
    vst1q_u16(out + i + 8, b);
  }
  for (; i < n; ++i)
    out[i] = src[i];
}

static inline uint32x4_t blend(uint32x4_t a, uint32x4_t b, uint32_t w) {
  if (!w)
    return a;
  if (w == 256)
    return b;
  const uint8x16_t aa = vreinterpretq_u8_u32(a), bb = vreinterpretq_u8_u32(b);
  const uint8x8_t wa = vdup_n_u8(256 - w), wb = vdup_n_u8(w);
  const uint16x8_t lo =
      vmlal_u8(vmull_u8(vget_low_u8(aa), wa), vget_low_u8(bb), wb);
  const uint16x8_t hi =
      vmlal_u8(vmull_u8(vget_high_u8(aa), wa), vget_high_u8(bb), wb);
  return vreinterpretq_u32_u8(
      vcombine_u8(vshrn_n_u16(lo, 8), vshrn_n_u16(hi, 8)));
}
static inline uint32_t scalar(uint32_t a, uint32_t b, uint32_t w) {
  uint32_t rb =
      (((a & 0x00ff00ff) * (256 - w) + (b & 0x00ff00ff) * w) >> 8) & 0x00ff00ff;
  uint32_t ga =
      ((((a >> 8) & 0x00ff00ff) * (256 - w) + ((b >> 8) & 0x00ff00ff) * w) >>
       8) &
      0x00ff00ff;
  return rb | (ga << 8);
}

void magik_launcher_shade_rgba(uint32_t *pixels, size_t n, uint32_t light) {
  size_t i = 0;
  // The Rust boundary skips full brightness; all remaining factors fit u8.
  const uint8x8_t factor = vdup_n_u8((uint8_t)light);
  const uint32x4_t alpha = vdupq_n_u32(0xff000000);
  for (; i + 4 <= n; i += 4) {
    const uint32x4_t original = vld1q_u32(pixels + i);
    const uint8x16_t bytes = vreinterpretq_u8_u32(original);
    const uint8x16_t shaded = vcombine_u8(
        vshrn_n_u16(vmull_u8(vget_low_u8(bytes), factor), 8),
        vshrn_n_u16(vmull_u8(vget_high_u8(bytes), factor), 8));
    vst1q_u32(pixels + i, vbslq_u32(alpha, original, vreinterpretq_u32_u8(shaded)));
  }
  for (; i < n; ++i) {
    const uint32_t p = pixels[i];
    const uint32_t rb = (((p & 0x00ff00ff) * light) >> 8) & 0x00ff00ff;
    const uint32_t g = (((p >> 8) & 255) * light >> 8) << 8;
    pixels[i] = (p & 0xff000000) | rb | g;
  }
}

static inline uint16x4_t reflect_channel(uint16x4_t a, uint16x4_t b,
                                        uint16x4_t w) {
  return vshr_n_u16(vmla_u16(vmul_u16(a, vsub_u16(vdup_n_u16(256), w)), b, w), 8);
}

static const uint8_t reflection_bayer[4][4] = {
    {0, 8, 2, 10}, {12, 4, 14, 6}, {3, 11, 1, 9}, {15, 7, 13, 5}};

static const uint16_t reflection_alpha_64[64] = {
    150, 145, 140, 136, 131, 127, 122, 118, 114, 110, 106, 102, 98,
    94,  90,  87,  83,  79,  76,  73,  69,  66,  63,  60,  57, 54,
    51,  48,  46,  43,  41,  38,  36,  34,  31,  29,  27,  25, 23,
    21,  19,  18,  16,  15,  13,  12,  10,  9,   8,   7,   6,  5,
    4,   3,   3,   2,   1,   1,   0,   0,   0,   0,   0,   0};

// Vector rows always begin at a multiple of eight, so one pattern per x
// phase covers all eight lanes.
static const uint16_t reflection_threshold_8[4][8] = {
    {8, 200, 56, 248, 8, 200, 56, 248},
    {136, 72, 184, 120, 136, 72, 184, 120},
    {40, 232, 24, 216, 40, 232, 24, 216},
    {168, 104, 152, 88, 168, 104, 152, 88}};

static inline uint32x4_t reverse4_u32(uint32x4_t value) {
  value = vrev64q_u32(value);
  return vcombine_u32(vget_high_u32(value), vget_low_u32(value));
}

static inline uint16x8_t reflection_fade(uint16x8_t channel,
                                         uint16x8_t alpha,
                                         uint16x8_t threshold) {
  const uint16x8_t value = vmulq_u16(channel, alpha);
  const uint16x8_t rounded = vandq_u16(vcgtq_u16(
      vandq_u16(value, vdupq_n_u16(255)), threshold), vdupq_n_u16(1));
  return vaddq_u16(vshrq_n_u16(value, 8), rounded);
}

static inline size_t reflection_fade_row(size_t row, size_t fade_rows) {
  size_t scaled;
  // Production card faces use 63 rows and the full-height fallback uses 64.
  // Keep both paths division-free on Cortex-A9; unusual test assets retain the
  // general contract.
  if (fade_rows == 64)
    scaled = row;
  else if (fade_rows == 63)
    scaled = row * 63 / 62;
  else
    scaled = row * 63 / (fade_rows - 1);
  return scaled < 63 ? scaled : 63;
}

void magik_launcher_prepare_reflection(uint16_t *out, const uint32_t *body,
                                       size_t height, size_t x, size_t fade_rows) {
  const size_t visible = height / 4 < 64 ? height / 4 : 64;
  const int standard_fade =
      height >= 64 && (fade_rows == 63 || fade_rows == 64);
  size_t row = 0;
  for (; row + 8 <= visible; row += 8) {
    const uint32x4_t first =
        reverse4_u32(vld1q_u32(body + height - row - 4));
    const uint32x4_t second =
        reverse4_u32(vld1q_u32(body + height - row - 8));
    const uint16x8_t red = vshrq_n_u16(
        vcombine_u16(vmovn_u32(vandq_u32(first, vdupq_n_u32(255))),
                     vmovn_u32(vandq_u32(second, vdupq_n_u32(255)))),
        3);
    const uint16x8_t green = vshrq_n_u16(
        vcombine_u16(
            vmovn_u32(vandq_u32(vshrq_n_u32(first, 8), vdupq_n_u32(255))),
            vmovn_u32(vandq_u32(vshrq_n_u32(second, 8), vdupq_n_u32(255)))),
        2);
    const uint16x8_t blue = vshrq_n_u16(
        vcombine_u16(
            vmovn_u32(vandq_u32(vshrq_n_u32(first, 16), vdupq_n_u32(255))),
            vmovn_u32(vandq_u32(vshrq_n_u32(second, 16), vdupq_n_u32(255)))),
        3);
    uint16x8_t alpha, threshold;
    if (standard_fade) {
      alpha = vld1q_u16(reflection_alpha_64 + row);
      threshold = vld1q_u16(reflection_threshold_8[x & 3]);
    } else {
      uint16_t alpha_values[8], threshold_values[8];
      for (size_t lane = 0; lane < 8; ++lane) {
        const size_t reflected_row =
            reflection_fade_row(row + lane, fade_rows);
        const uint32_t left = 63 - reflected_row;
        alpha_values[lane] = (uint16_t)(150 * left * left / (63 * 63));
        threshold_values[lane] =
            (uint16_t)(reflection_bayer[reflected_row & 3][x & 3] * 16 + 8);
      }
      alpha = vld1q_u16(alpha_values);
      threshold = vld1q_u16(threshold_values);
    }
    const uint16x8_t faded_red = reflection_fade(red, alpha, threshold);
    const uint16x8_t faded_green = reflection_fade(green, alpha, threshold);
    const uint16x8_t faded_blue = reflection_fade(blue, alpha, threshold);
    vst1q_u16(out + row,
              vorrq_u16(vorrq_u16(vshlq_n_u16(faded_red, 11),
                                  vshlq_n_u16(faded_green, 5)),
                        faded_blue));
  }
  for (; row < 64; ++row) {
    const uint32_t pixel = row < visible ? body[height - 1 - row] : 0;
    const size_t fade_row = reflection_fade_row(row, fade_rows);
    const uint32_t left = fade_row < 63 ? 63 - fade_row : 0;
    const uint32_t alpha = 150 * left * left / (63 * 63);
    const uint32_t threshold = reflection_bayer[fade_row & 3][x & 3] * 16 + 8;
    const uint32_t red = (pixel & 255) >> 3;
    const uint32_t green = ((pixel >> 8) & 255) >> 2;
    const uint32_t blue = ((pixel >> 16) & 255) >> 3;
#define FADE(channel)                                                          \
  (((channel) * alpha) / 256 + (((channel) * alpha) % 256 > threshold))
    out[row] = (uint16_t)(FADE(red) << 11 | FADE(green) << 5 | FADE(blue));
#undef FADE
  }
}

void magik_launcher_reflect_column(uint16_t *out, size_t pitch,
                                   const uint16_t *src, size_t height,
                                   size_t rows, int32_t q, int32_t step) {
  size_t y = 0;
  while (y < rows) {
    int32_t r = q >> 16, next = q + step, r2 = next >> 16;
    if (y + 1 < rows && r >= 0 && r2 >= 0 &&
        (size_t)(r + 3) < height && (size_t)(r2 + 3) < height) {
      // Two adjacent pairs; the extra two lanes are bounded lookahead and
      // discarded. Each RGB565 channel retains its own exact integer floor.
      uint16x4x2_t ab = vtrn_u16(vld1_u16(src + r), vld1_u16(src + r2));
      uint16x4_t w = vset_lane_u16(((uint32_t)next & 65535) >> 8,
                                   vdup_n_u16(((uint32_t)q & 65535) >> 8), 1);
      uint16x4_t red = reflect_channel(vshr_n_u16(ab.val[0], 11), vshr_n_u16(ab.val[1], 11), w);
      uint16x4_t green = reflect_channel(vand_u16(vshr_n_u16(ab.val[0], 5), vdup_n_u16(63)), vand_u16(vshr_n_u16(ab.val[1], 5), vdup_n_u16(63)), w);
      uint16x4_t blue = reflect_channel(vand_u16(ab.val[0], vdup_n_u16(31)), vand_u16(ab.val[1], vdup_n_u16(31)), w);
      uint16x4_t pixels = vorr_u16(vorr_u16(vshl_n_u16(red, 11), vshl_n_u16(green, 5)), blue);
      vst1_lane_u16(out + y * pitch, pixels, 0);
      vst1_lane_u16(out + (y + 1) * pitch, pixels, 1);
      y += 2; q = next + step;
    } else {
      uint32_t a = r >= 0 && (size_t)r < height ? src[r] : 0;
      uint32_t b = r + 1 >= 0 && (size_t)(r + 1) < height ? src[r + 1] : 0;
      uint32_t w = ((uint32_t)q & 65535) >> 8;
      uint32_t red = ((a >> 11) * (256 - w) + (b >> 11) * w) >> 8;
      uint32_t green = (((a >> 5) & 63) * (256 - w) + ((b >> 5) & 63) * w) >> 8;
      uint32_t blue = ((a & 31) * (256 - w) + (b & 31) * w) >> 8;
      out[y * pitch] = (uint16_t)(red << 11 | green << 5 | blue);
      ++y; q = next;
    }
  }
}
void magik_launcher_filter_column(uint32_t *out, const uint32_t *a0,
                                  const uint32_t *a1, const uint32_t *b0,
                                  const uint32_t *b1, size_t n, uint32_t wx,
                                  uint32_t wx2, uint32_t lod) {
  size_t i = 0;
  for (; i + 4 <= n; i += 4) {
    uint32x4_t a = blend(vld1q_u32(a0 + i), vld1q_u32(a1 + i), wx);
    if (lod)
      a = blend(a, blend(vld1q_u32(b0 + i), vld1q_u32(b1 + i), wx2), lod);
    vst1q_u32(out + i, a);
  }
  for (; i < n; ++i) {
    uint32_t a = scalar(a0[i], a1[i], wx);
    out[i] = lod ? scalar(a, scalar(b0[i], b1[i], wx2), lod) : a;
  }
}

static uint16_t over_pixel(uint32_t p, uint16_t dst) {
  uint32_t alpha = p >> 24;
  if (!alpha)
    return dst;
  if (alpha == 255)
    return (uint16_t)(((p & 248) << 8) | (((p >> 8) & 252) << 3) |
                      ((p >> 19) & 31));
  uint32_t r = (p & 255) + ((dst >> 11) * 255 / 31) * (255 - alpha) / 255;
  uint32_t g =
      ((p >> 8) & 255) + (((dst >> 5) & 63) * 255 / 63) * (255 - alpha) / 255;
  uint32_t b =
      ((p >> 16) & 255) + ((dst & 31) * 255 / 31) * (255 - alpha) / 255;
  if (r > 255)
    r = 255;
  if (g > 255)
    g = 255;
  if (b > 255)
    b = 255;
  return (uint16_t)((r >> 3) << 11 | (g >> 2) << 5 | b >> 3);
}

static inline uint32x2_t pack_opaque2(uint32x2_t p) {
  uint32x2_t red = vshl_n_u32(vand_u32(p, vdup_n_u32(248)), 8);
  uint32x2_t green =
      vshl_n_u32(vand_u32(vshr_n_u32(p, 8), vdup_n_u32(252)), 3);
  uint32x2_t blue = vand_u32(vshr_n_u32(p, 19), vdup_n_u32(31));
  return vorr_u32(vorr_u32(red, green), blue);
}

static inline uint32x2_t interpolate2(const uint32_t *src, int32_t q0,
                                      int32_t q1) {
  int32_t r0 = q0 >> 16, r1 = q1 >> 16;
  uint32x2x2_t ab = vtrn_u32(vld1_u32(src + r0), vld1_u32(src + r1));
  uint16x8_t a = vmovl_u8(vreinterpret_u8_u32(ab.val[0]));
  uint16x8_t b = vmovl_u8(vreinterpret_u8_u32(ab.val[1]));
  uint16x8_t w = vcombine_u16(vdup_n_u16(((uint32_t)q0 & 65535) >> 8),
                                vdup_n_u16(((uint32_t)q1 & 65535) >> 8));
  return vreinterpret_u32_u8(vshrn_n_u16(
      vmlaq_u16(vmulq_u16(a, vsubq_u16(vdupq_n_u16(256), w)), b, w), 8));
}

// Exact two-row perspective interpolation composed directly to RGB565.
// Avoids writing and then rereading a projected RGBA image for each card.
void magik_launcher_project_over_column(uint16_t *out, size_t pitch,
                                       const uint32_t *src, size_t height,
                                       size_t rows, int32_t q, int32_t step,
                                       size_t opaque_top,
                                       size_t opaque_bottom) {
  size_t y = 0;
  const int opaque_interior =
      opaque_top < opaque_bottom && opaque_bottom <= height &&
      (src[opaque_top] >> 24) == 255 &&
      (src[opaque_bottom - 1] >> 24) == 255;
  while (y < rows) {
    int32_t r = q >> 16, next = q + step, r2 = next >> 16;
    // Uniform opaque runs need neither interpolation nor destination reads.
    // Stop before the final texel so both bilinear inputs remain identical.
    if (r >= 0 && (size_t)(r + 3) < height &&
        (src[r] >> 24) == 255 && src[r] == src[r+1] &&
        src[r] == src[r+2] && src[r] == src[r+3]) {
      size_t end = (size_t)r + 4;
      uint32_t pixel = src[r];
      while (end < height && src[end] == pixel) ++end;
      uint16_t packed = over_pixel(pixel, 0);
      while (y < rows && (size_t)(q >> 16) + 1 < end) {
        out[y * pitch] = packed;
        ++y; q += step;
      }
      continue;
    }
    if (opaque_interior && y + 7 < rows) {
      int32_t q7 = q + 7 * step;
      int32_t last = q7 >> 16;
      if (r >= 0 && (size_t)r >= opaque_top &&
          (size_t)(last + 1) < opaque_bottom) {
        for (size_t pair = 0; pair < 4; ++pair) {
          int32_t q0 = q + (int32_t)(pair * 2) * step;
          int32_t q1 = q0 + step;
          uint32x2_t packed = pack_opaque2(interpolate2(src, q0, q1));
          out[(y + pair * 2) * pitch] =
              (uint16_t)vget_lane_u32(packed, 0);
          out[(y + pair * 2 + 1) * pitch] =
              (uint16_t)vget_lane_u32(packed, 1);
        }
        y += 8;
        q += 8 * step;
        continue;
      }
    }
    // Photographic faces rarely contain equal-colour vertical runs, but their
    // interior is still opaque. Interpolate and pack four output rows per
    // iteration so that opaque artwork avoids destination reads and halves
    // the loop/control overhead of the generic two-row path.
    if (y + 3 < rows && r >= 0 && r2 >= 0) {
      int32_t q2 = next + step, q3 = q2 + step;
      int32_t r3 = q2 >> 16, r4 = q3 >> 16;
      if (r3 >= 0 && r4 >= 0 && (size_t)(r + 1) < height &&
          (size_t)(r2 + 1) < height && (size_t)(r3 + 1) < height &&
          (size_t)(r4 + 1) < height) {
        uint32x2_t p01 = interpolate2(src, q, next);
        uint32x2_t p23 = interpolate2(src, q2, q3);
        uint32x2_t a01 = vshr_n_u32(p01, 24);
        uint32x2_t a23 = vshr_n_u32(p23, 24);
        uint32x2_t minimum = vmin_u32(a01, a23);
        if (vget_lane_u32(minimum, 0) == 255 &&
            vget_lane_u32(minimum, 1) == 255) {
          uint32x2_t packed01 = pack_opaque2(p01);
          uint32x2_t packed23 = pack_opaque2(p23);
          out[y * pitch] = (uint16_t)vget_lane_u32(packed01, 0);
          out[(y + 1) * pitch] = (uint16_t)vget_lane_u32(packed01, 1);
          out[(y + 2) * pitch] = (uint16_t)vget_lane_u32(packed23, 0);
          out[(y + 3) * pitch] = (uint16_t)vget_lane_u32(packed23, 1);
          y += 4;
          q = q3 + step;
          continue;
        }
      }
    }
    if (y + 1 < rows && r >= 0 && r2 >= 0 &&
        (size_t)(r + 1) < height && (size_t)(r2 + 1) < height) {
      uint32x2_t p = interpolate2(src, q, next);
      if ((vget_lane_u32(p, 0) >> 24) == 255 &&
          (vget_lane_u32(p, 1) >> 24) == 255) {
        uint32x2_t packed = pack_opaque2(p);
        out[y * pitch] = (uint16_t)vget_lane_u32(packed, 0);
        out[(y + 1) * pitch] = (uint16_t)vget_lane_u32(packed, 1);
      } else {
        out[y * pitch] = over_pixel(vget_lane_u32(p, 0), out[y * pitch]);
        out[(y + 1) * pitch] = over_pixel(vget_lane_u32(p, 1), out[(y + 1) * pitch]);
      }
      y += 2; q = next + step;
    } else {
      uint32_t a = r >= 0 && (size_t)r < height ? src[r] : 0;
      uint32_t b = r + 1 >= 0 && (size_t)(r + 1) < height ? src[r + 1] : 0;
      out[y * pitch] = over_pixel(scalar(a, b, ((uint32_t)q & 65535) >> 8), out[y * pitch]);
      ++y; q = next;
    }
  }
}
// Face-on cards share one vertical mapping. Gather four column pairs and
// composite immediately, avoiding a strided intermediate image and reread.
void magik_launcher_flat(uint16_t *out, size_t pitch, const uint32_t *src,
                        size_t stride, size_t height, size_t width, size_t rows,
                        int32_t q, int32_t step) {
  for (size_t y = 0; y < rows; ++y, q += step) {
    int32_t r = q >> 16;
    uint32_t w = ((uint32_t)q & 65535) >> 8;
    size_t x = 0;
    if (r >= 0 && (size_t)(r + 1) < height) {
      for (; x + 4 <= width; x += 4) {
        uint32x2x2_t ab = vtrn_u32(vld1_u32(src + x * stride + r),
                                   vld1_u32(src + (x + 1) * stride + r));
        uint32x2x2_t cd = vtrn_u32(vld1_u32(src + (x + 2) * stride + r),
                                   vld1_u32(src + (x + 3) * stride + r));
        uint32x4_t p = blend(vcombine_u32(ab.val[0], cd.val[0]),
                             vcombine_u32(ab.val[1], cd.val[1]), w);
        uint32x4_t alpha = vshrq_n_u32(p, 24);
        uint32x2_t m = vmin_u32(vget_low_u32(alpha), vget_high_u32(alpha));
        if (vget_lane_u32(m, 0) == 255 && vget_lane_u32(m, 1) == 255) {
          uint32x4_t red = vshlq_n_u32(vandq_u32(p, vdupq_n_u32(248)), 8);
          uint32x4_t green = vshlq_n_u32(vandq_u32(vshrq_n_u32(p, 8), vdupq_n_u32(252)), 3);
          uint32x4_t blue = vandq_u32(vshrq_n_u32(p, 19), vdupq_n_u32(31));
          vst1_u16(out + y * pitch + x, vmovn_u32(vorrq_u32(vorrq_u32(red, green), blue)));
        } else {
          uint32_t pixels[4];
          vst1q_u32(pixels, p);
          for (size_t j = 0; j < 4; ++j)
            out[y * pitch + x + j] = over_pixel(pixels[j], out[y * pitch + x + j]);
        }
      }
    }
    for (; x < width; ++x) {
      uint32_t a = r >= 0 && (size_t)r < height ? src[x * stride + r] : 0;
      uint32_t b = r + 1 >= 0 && (size_t)(r + 1) < height ? src[x * stride + r + 1] : 0;
      out[y * pitch + x] = over_pixel(scalar(a, b, w), out[y * pitch + x]);
    }
  }
}

void magik_launcher_mix_rgba(uint32_t *a, const uint32_t *b, size_t n, uint32_t w) {
  size_t i = 0;
  for (; i + 4 <= n; i += 4)
    vst1q_u32(a + i, blend(vld1q_u32(a + i), vld1q_u32(b + i), w));
  for (; i < n; ++i) a[i] = scalar(a[i], b[i], w);
}

static uint16_t unpremultiply(uint32_t p) {
  uint32_t alpha = p >> 24;
  if (!alpha)
    return 0;
  uint32_t r = (p & 255) * 255 / alpha, g = ((p >> 8) & 255) * 255 / alpha,
           b = ((p >> 16) & 255) * 255 / alpha;
  if (r > 255)
    r = 255;
  if (g > 255)
    g = 255;
  if (b > 255)
    b = 255;
  return (uint16_t)((r >> 3) << 11 | (g >> 2) << 5 | b >> 3);
}

static uint16x8_t colour_channel(uint16x8_t a, uint16x8_t b, uint16_t weight) {
  return vshrq_n_u16(vmlaq_n_u16(vmulq_n_u16(a, 256 - weight), b, weight), 8);
}
void magik_launcher_blend_row(uint16_t *out, const uint16_t *a,
                              const uint16_t *b, size_t n, uint16_t weight) {
  size_t i = 0;
  for (; i + 8 <= n; i += 8) {
    uint16x8_t aa = vld1q_u16(a + i), bb = vld1q_u16(b + i);
    uint16x8_t r =
        colour_channel(vshrq_n_u16(aa, 11), vshrq_n_u16(bb, 11), weight);
    uint16x8_t g =
        colour_channel(vandq_u16(vshrq_n_u16(aa, 5), vdupq_n_u16(63)),
                       vandq_u16(vshrq_n_u16(bb, 5), vdupq_n_u16(63)), weight);
    uint16x8_t blue = colour_channel(vandq_u16(aa, vdupq_n_u16(31)),
                                     vandq_u16(bb, vdupq_n_u16(31)), weight);
    vst1q_u16(
        out + i,
        vorrq_u16(vorrq_u16(vshlq_n_u16(r, 11), vshlq_n_u16(g, 5)), blue));
  }
  for (; i < n; ++i) {
    uint32_t r = ((a[i] >> 11) * (256 - weight) + (b[i] >> 11) * weight) >> 8;
    uint32_t g =
        (((a[i] >> 5) & 63) * (256 - weight) + ((b[i] >> 5) & 63) * weight) >>
        8;
    uint32_t blue = ((a[i] & 31) * (256 - weight) + (b[i] & 31) * weight) >> 8;
    out[i] = (uint16_t)(r << 11 | g << 5 | blue);
  }
}

void magik_launcher_raster_row(uint16_t *out, uint8_t *alpha, const uint32_t *a,
                               const uint32_t *b, size_t n, uint32_t weight) {
  size_t i = 0;
  for (; i + 4 <= n; i += 4) {
    uint32x4_t p = blend(vld1q_u32(a + i), vld1q_u32(b + i), weight);
    uint32x4_t coverage = vshrq_n_u32(p, 24);
    uint32x2_t m = vmin_u32(vget_low_u32(coverage), vget_high_u32(coverage));
    if (vget_lane_u32(m, 0) == 255 && vget_lane_u32(m, 1) == 255) {
      uint32x4_t r = vshlq_n_u32(vandq_u32(p, vdupq_n_u32(248)), 8);
      uint32x4_t g =
          vshlq_n_u32(vandq_u32(vshrq_n_u32(p, 8), vdupq_n_u32(252)), 3);
      uint32x4_t blue = vandq_u32(vshrq_n_u32(p, 19), vdupq_n_u32(31));
      vst1_u16(out + i, vmovn_u32(vorrq_u32(vorrq_u32(r, g), blue)));
      alpha[i] = alpha[i + 1] = alpha[i + 2] = alpha[i + 3] = 255;
    } else {
      uint32_t samples[4];
      vst1q_u32(samples, p);
      for (size_t j = 0; j < 4; ++j) {
        alpha[i + j] = samples[j] >> 24;
        out[i + j] = unpremultiply(samples[j]);
      }
    }
  }
  for (; i < n; ++i) {
    uint32_t p = scalar(a[i], b[i], weight);
    alpha[i] = p >> 24;
    out[i] = unpremultiply(p);
  }
}

// Experimental final quantisation, after perspective filtering and lighting.
static const uint32_t image_threshold[4][4] = {
  {8,136,40,168},{200,72,232,104},{56,184,24,152},{248,120,216,88}};
// Exact ordered rounding collapses to one division: with 0<t<256,
// remainder*256 > t*255 iff remainder >= t. Thus round(s/255,t)
// is floor((s+255-t)/255). For this bounded range, division by 255 is
// (n+1+((n+1)>>8))>>8. All intermediate values fit sixteen bits.
static inline uint16_t quantise_image_scalar(uint32_t value,uint32_t levels,uint32_t offset) {
  uint32_t n=value*levels+offset;
  return (uint16_t)((n+(n>>8))>>8);
}
static inline uint16_t pack_dithered_scalar(uint32_t p,size_t x,size_t y) {
  uint32_t offset=256-image_threshold[y&3][x&3];
  return (uint16_t)(quantise_image_scalar(p&255,31,offset)<<11 |
    quantise_image_scalar((p>>8)&255,63,offset)<<5 |
    quantise_image_scalar((p>>16)&255,31,offset));
}
static inline uint16x4_t quantise_image16(uint16x4_t value,uint16_t levels,uint16x4_t offset) {
  uint16x4_t n=vmla_n_u16(offset,value,levels);
  return vshr_n_u16(vadd_u16(n,vshr_n_u16(n,8)),8);
}
static inline uint16x4_t pack_dithered16(uint32x4_t p,uint16x4_t offset) {
  const uint32x4_t mask=vdupq_n_u32(255);
  uint16x4_t r=quantise_image16(vmovn_u32(vandq_u32(p,mask)),31,offset);
  uint16x4_t g=quantise_image16(vmovn_u32(vandq_u32(vshrq_n_u32(p,8),mask)),63,offset);
  uint16x4_t b=quantise_image16(vmovn_u32(vandq_u32(vshrq_n_u32(p,16),mask)),31,offset);
  return vorr_u16(vorr_u16(vshl_n_u16(r,11),vshl_n_u16(g,5)),b);
}
// Exact four-pixel premultiplied over. RGB565 background decoding uses
// floor(value*255/levels), and division by 255 preserves the scalar floor.
static inline uint16x4_t dither_decode5(uint16x4_t v) {
  uint16x4_t n=vmla_n_u16(vdup_n_u16(1),v,7);
  return vadd_u16(vshl_n_u16(v,3),vshr_n_u16(vadd_u16(n,vshr_n_u16(n,5)),5));
}
static inline uint16x4_t dither_decode6(uint16x4_t v) {
  uint16x4_t n=vmla_n_u16(vdup_n_u16(1),v,3);
  return vadd_u16(vshl_n_u16(v,2),vshr_n_u16(vadd_u16(n,vshr_n_u16(n,6)),6));
}
static inline uint16x4_t dither_over_channel4(uint16x4_t source,uint16x4_t bg,uint16x4_t inverse) {
  uint16x4_t n=vadd_u16(vmul_u16(bg,inverse),vdup_n_u16(1));
  uint16x4_t over=vadd_u16(source,vshr_n_u16(vadd_u16(n,vshr_n_u16(n,8)),8));
  return vmin_u16(over,vdup_n_u16(255));
}
static inline uint16x4_t dither_over4(uint32x4_t p,uint16x4_t dst,uint16x4_t offset) {
  const uint32x4_t mask=vdupq_n_u32(255);
  uint16x4_t alpha=vmovn_u32(vshrq_n_u32(p,24));
  uint16x4_t inverse=vsub_u16(vdup_n_u16(255),alpha);
  uint16x4_t r=dither_over_channel4(vmovn_u32(vandq_u32(p,mask)),dither_decode5(vshr_n_u16(dst,11)),inverse);
  uint16x4_t g=dither_over_channel4(vmovn_u32(vandq_u32(vshrq_n_u32(p,8),mask)),dither_decode6(vand_u16(vshr_n_u16(dst,5),vdup_n_u16(63))),inverse);
  uint16x4_t b=dither_over_channel4(vmovn_u32(vandq_u32(vshrq_n_u32(p,16),mask)),dither_decode5(vand_u16(dst,vdup_n_u16(31))),inverse);
  r=quantise_image16(r,31,offset);g=quantise_image16(g,63,offset);b=quantise_image16(b,31,offset);
  uint16x4_t packed=vorr_u16(vorr_u16(vshl_n_u16(r,11),vshl_n_u16(g,5)),b);
  return vbsl_u16(vceq_u16(alpha,vdup_n_u16(0)),dst,packed);
}
static uint16_t dither_pixel(uint32_t p, uint16_t dst, size_t x, size_t y) {
  uint32_t a=p>>24;
  if(!a) return dst;
  if(a==255) return pack_dithered_scalar(p,x,y);
  uint32_t channels[3]={p&255,(p>>8)&255,(p>>16)&255};
  uint32_t bg[3]={(dst>>11)*255/31,((dst>>5)&63)*255/63,(dst&31)*255/31};
  for(size_t c=0;c<3;++c) {
    uint32_t v=channels[c]+bg[c]*(255-a)/255;
    channels[c]=v>255?255:v;
  }
  return pack_dithered_scalar(channels[0]|channels[1]<<8|channels[2]<<16,x,y);
}
static inline uint32x2_t pack_dithered2(uint32x2_t p,size_t x,size_t y) {
  uint16_t offsets[4]={(uint16_t)(256-image_threshold[y&3][x&3]),
    (uint16_t)(256-image_threshold[(y+1)&3][x&3]),0,0};
  uint16x4_t packed=pack_dithered16(vcombine_u32(p,vdup_n_u32(0)),vld1_u16(offsets));
  return vget_low_u32(vmovl_u16(packed));
}
void magik_launcher_project_dithered(uint16_t *out,size_t pitch,const uint32_t *src,
    size_t height,size_t rows,int32_t q,int32_t step,size_t x,size_t y0) {
  size_t y=0;
  // Four vertical outputs fill every quantiser lane. The Bayer phase repeats
  // after four rows, so these offsets stay outside the interior loop.
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j) offsets[j]=(uint16_t)(256-image_threshold[(y0+j)&3][x&3]);
  const uint16x4_t phase=vld1_u16(offsets);
  for(;y+3<rows;y+=4) {
    int32_t q1=q+step,q2=q1+step,q3=q2+step;
    int32_t r=q>>16,r3=q3>>16;
    if(r>=0 && (size_t)(r3+1)<height) {
      uint32x4_t p=vcombine_u32(interpolate2(src,q,q1),interpolate2(src,q2,q3));
      uint32x4_t alpha=vshrq_n_u32(p,24);
      uint32x2_t minimum=vmin_u32(vget_low_u32(alpha),vget_high_u32(alpha));
      if(vget_lane_u32(minimum,0)==255 && vget_lane_u32(minimum,1)==255) {
        uint16x4_t packed=pack_dithered16(p,phase);
        out[y*pitch]=vget_lane_u16(packed,0);
        out[(y+1)*pitch]=vget_lane_u16(packed,1);
        out[(y+2)*pitch]=vget_lane_u16(packed,2);
        out[(y+3)*pitch]=vget_lane_u16(packed,3);
      } else {
        uint32_t pixels[4];vst1q_u32(pixels,p);
        for(size_t j=0;j<4;++j) out[(y+j)*pitch]=dither_pixel(pixels[j],out[(y+j)*pitch],x,y0+y+j);
      }
    } else {
      int32_t qj=q;
      for(size_t j=0;j<4;++j,qj+=step) {
        int32_t rj=qj>>16;
        uint32_t a=rj>=0 && (size_t)rj<height?src[rj]:0;
        uint32_t b=rj+1>=0 && (size_t)(rj+1)<height?src[rj+1]:0;
        out[(y+j)*pitch]=dither_pixel(scalar(a,b,((uint32_t)qj&65535)>>8),out[(y+j)*pitch],x,y0+y+j);
      }
    }
    q=q3+step;
  }
  for (;y+1<rows;y+=2) {
    int32_t r=q>>16, q1=q+step, r1=q1>>16;
    if (r>=0 && r1>=0 && (size_t)(r+1)<height && (size_t)(r1+1)<height) {
      uint32x2_t p=interpolate2(src,q,q1);
      uint32x2_t alpha=vshr_n_u32(p,24);
      if (vget_lane_u32(alpha,0)==255 && vget_lane_u32(alpha,1)==255) {
        uint32x2_t packed=pack_dithered2(p,x,y0+y);
        out[y*pitch]=(uint16_t)vget_lane_u32(packed,0);
        out[(y+1)*pitch]=(uint16_t)vget_lane_u32(packed,1);
      } else {
        out[y*pitch]=dither_pixel(vget_lane_u32(p,0),out[y*pitch],x,y0+y);
        out[(y+1)*pitch]=dither_pixel(vget_lane_u32(p,1),out[(y+1)*pitch],x,y0+y+1);
      }
    } else {
      for (size_t j=0;j<2;++j) {
        int32_t qj=q+(int32_t)j*step,rj=qj>>16;
        uint32_t a=rj>=0 && (size_t)rj<height ? src[rj]:0;
        uint32_t b=rj+1>=0 && (size_t)(rj+1)<height ? src[rj+1]:0;
        out[(y+j)*pitch]=dither_pixel(scalar(a,b,((uint32_t)qj&65535)>>8),out[(y+j)*pitch],x,y0+y+j);
      }
    }
    q=q1+step;
  }
  if (y<rows) {
    int32_t r=q>>16;
    uint32_t a=r>=0 && (size_t)r<height ? src[r]:0;
    uint32_t b=r+1>=0 && (size_t)(r+1)<height ? src[r+1]:0;
    out[y*pitch]=dither_pixel(scalar(a,b,((uint32_t)q&65535)>>8),out[y*pitch],x,y0+y);
  }
}

// Flat cards use contiguous four-pixel stores, as in the production flat path.
static inline uint16x4_t pack_dithered4(uint32x4_t p,size_t x,size_t y) {
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j) offsets[j]=(uint16_t)(256-image_threshold[y&3][(x+j)&3]);
  return pack_dithered16(p,vld1_u16(offsets));
}
void magik_launcher_flat_dithered(uint16_t *out, size_t pitch,
    const uint32_t *src, size_t stride, size_t height, size_t width,
    size_t rows, int32_t q, int32_t step, size_t x0, size_t y0) {
  for (size_t y = 0; y < rows; ++y, q += step) {
    int32_t r = q >> 16;
    uint32_t w = ((uint32_t)q & 65535) >> 8;
    size_t x = 0;
    if (r >= 0 && (size_t)(r + 1) < height) {
      for (; x + 3 < width; x += 4) {
        const uint32_t *p0 = src + x * stride + r;
        const uint32_t *p1 = p0 + stride, *p2 = p1 + stride, *p3 = p2 + stride;
        uint32x2x2_t a = vtrn_u32(vld1_u32(p0), vld1_u32(p1));
        uint32x2x2_t b = vtrn_u32(vld1_u32(p2), vld1_u32(p3));
        uint32x4_t p = blend(vcombine_u32(a.val[0], b.val[0]),
                            vcombine_u32(a.val[1], b.val[1]), w);
        uint32x4_t alpha = vshrq_n_u32(p, 24);
        uint32x2_t m = vmin_u32(vget_low_u32(alpha), vget_high_u32(alpha));
        if (vget_lane_u32(m, 0) == 255 && vget_lane_u32(m, 1) == 255) {
          vst1_u16(out + y * pitch + x, pack_dithered4(p, x0 + x, y0 + y));
        } else {
          uint32_t pixels[4];
          vst1q_u32(pixels, p);
          for (size_t j = 0; j < 4; ++j)
            out[y * pitch + x + j] = dither_pixel(pixels[j], out[y * pitch + x + j], x0 + x + j, y0 + y);
        }
      }
    }
    for (; x < width; ++x) {
      uint32_t a = r >= 0 && (size_t)r < height ? src[x * stride + r] : 0;
      uint32_t b = r + 1 >= 0 && (size_t)(r + 1) < height ? src[x * stride + r + 1] : 0;
      out[y * pitch + x] = dither_pixel(scalar(a, b, w), out[y * pitch + x], x0 + x, y0 + y);
    }
  }
}

// Separable cabinet rows. These bounded gathers are per horizontal row, while
// vertical filtering, mip blending and output use contiguous four-pixel batches.
typedef struct { int16_t index; uint16_t weight; } cabinet_column;
// Each map weight is 0..255. VQDMULH(delta, weight<<7) is exactly
// floor(delta*weight/256), including negative deltas, without saturation.
// Keep the two byte channels in each halfword separate until the final pack.
static inline uint32x4_t cabinet_mix4(uint32x4_t a,uint32x4_t b,uint32x4_t weight) {
  uint16x8_t w=vreinterpretq_u16_u32(weight);
  int16x8_t coefficient=vreinterpretq_s16_u16(vshlq_n_u16(vtrnq_u16(w,w).val[0],7));
  uint16x8_t aa=vreinterpretq_u16_u32(a),bb=vreinterpretq_u16_u32(b),mask=vdupq_n_u16(255);
  int16x8_t al=vreinterpretq_s16_u16(vandq_u16(aa,mask)),bl=vreinterpretq_s16_u16(vandq_u16(bb,mask));
  int16x8_t ah=vreinterpretq_s16_u16(vshrq_n_u16(aa,8)),bh=vreinterpretq_s16_u16(vshrq_n_u16(bb,8));
  uint16x8_t lo=vreinterpretq_u16_s16(vaddq_s16(al,vqdmulhq_s16(vsubq_s16(bl,al),coefficient)));
  uint16x8_t hi=vreinterpretq_u16_s16(vaddq_s16(ah,vqdmulhq_s16(vsubq_s16(bh,ah),coefficient)));
  return vreinterpretq_u32_u16(vorrq_u16(lo,vshlq_n_u16(hi,8)));
}
static inline uint32_t cabinet_border(const uint32_t *source,size_t width,cabinet_column column) {
  int32_t ix=column.index;
  uint32_t a=ix>=0 && (size_t)ix<width ? source[ix]:0;
  uint32_t b=ix+1>=0 && (size_t)(ix+1)<width ? source[ix+1]:0;
  return scalar(a,b,column.weight);
}
void magik_cabinet_horizontal(uint32_t *out,const uint32_t *source,size_t width,const cabinet_column *columns,size_t n,size_t first,size_t end) {
  size_t i=0;
  for(;i<first;++i) out[i]=cabinet_border(source,width,columns[i]);
  // The monotonic mapping's interior is checked once per frame by Rust.
  // Load each neighbouring RGBA pair together, then transpose the four pairs.
  for(;i+3<end;i+=4) {
    uint16x4x2_t map=vld2_u16((const uint16_t *)(columns+i));
    const uint32_t *p0=source+vget_lane_u16(map.val[0],0);
    const uint32_t *p1=source+vget_lane_u16(map.val[0],1);
    const uint32_t *p2=source+vget_lane_u16(map.val[0],2);
    const uint32_t *p3=source+vget_lane_u16(map.val[0],3);
    uint32x2x2_t a=vtrn_u32(vld1_u32(p0),vld1_u32(p1));
    uint32x2x2_t b=vtrn_u32(vld1_u32(p2),vld1_u32(p3));
    vst1q_u32(out+i,cabinet_mix4(vcombine_u32(a.val[0],b.val[0]),
      vcombine_u32(a.val[1],b.val[1]),vmovl_u16(map.val[1])));
  }
  for(;i<n;++i) out[i]=cabinet_border(source,width,columns[i]);
}
static void cabinet_composite_black(uint16_t *out,const uint32_t *a,const uint32_t *b,const uint32_t *c,const uint32_t *d,size_t n,uint32_t wy,uint32_t wy2,uint32_t lod,size_t x,size_t y) {
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j) offsets[j]=(uint16_t)(256-image_threshold[y&3][(x+j)&3]);
  const uint16x4_t phase=vld1_u16(offsets),zero=vdup_n_u16(0);
  size_t i=0;
  for(;i+3<n;i+=4) {
    uint32x4_t p=blend(vld1q_u32(a+i),vld1q_u32(b+i),wy);
    if(lod) p=blend(p,blend(vld1q_u32(c+i),vld1q_u32(d+i),wy2),lod);
    uint32x4_t alpha=vshrq_n_u32(p,24);
    uint32x2_t maximum=vmax_u32(vget_low_u32(alpha),vget_high_u32(alpha));
    if(vget_lane_u32(maximum,0)==0 && vget_lane_u32(maximum,1)==0) continue;
    uint16x4_t packed=pack_dithered16(p,phase);
    vst1_u16(out+i,vbsl_u16(vceq_u16(vmovn_u32(alpha),zero),zero,packed));
  }
  for(;i<n;++i) {
    uint32_t p=scalar(a[i],b[i],wy);
    if(lod) p=scalar(p,scalar(c[i],d[i],wy2),lod);
    out[i]=(p>>24)?pack_dithered_scalar(p,x+i,y):0;
  }
}

void magik_cabinet_composite(uint16_t *out,const uint32_t *a,const uint32_t *b,const uint32_t *c,const uint32_t *d,size_t n,uint32_t wy,uint32_t wy2,uint32_t lod,size_t x,size_t y,uint32_t black) {
  if(black) {cabinet_composite_black(out,a,b,c,d,n,wy,wy2,lod,x,y);return;}
  uint16_t offsets[4];
  for(size_t j=0;j<4;++j) offsets[j]=(uint16_t)(256-image_threshold[y&3][(x+j)&3]);
  const uint16x4_t phase=vld1_u16(offsets);
  size_t i=0;
  for(;i+3<n;i+=4) {
    uint32x4_t p=blend(vld1q_u32(a+i),vld1q_u32(b+i),wy);
    if(lod) p=blend(p,blend(vld1q_u32(c+i),vld1q_u32(d+i),wy2),lod);
    uint32x4_t alpha=vshrq_n_u32(p,24);
    uint32x2_t maximum=vmax_u32(vget_low_u32(alpha),vget_high_u32(alpha));
    if(vget_lane_u32(maximum,0)==0 && vget_lane_u32(maximum,1)==0) continue;
    uint32x2_t m=vmin_u32(vget_low_u32(alpha),vget_high_u32(alpha));
    if(vget_lane_u32(m,0)==255 && vget_lane_u32(m,1)==255) vst1_u16(out+i,pack_dithered16(p,phase));
    else vst1_u16(out+i,dither_over4(p,vld1_u16(out+i),phase));
  }
  for(;i<n;++i) {
    uint32_t p=scalar(a[i],b[i],wy);
    if(lod) p=scalar(p,scalar(c[i],d[i],wy2),lod);
    out[i]=dither_pixel(p,out[i],x+i,y);
  }
}
