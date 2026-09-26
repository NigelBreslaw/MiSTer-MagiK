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

void magik_launcher_prepare_reflection(uint16_t *out, const uint32_t *body,
                                       size_t height, size_t x, size_t fade_rows) {
  const size_t visible = height / 4 < 64 ? height / 4 : 64;
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
    uint16_t alpha_values[8], threshold_values[8];
    for (size_t lane = 0; lane < 8; ++lane) {
      const size_t reflected_row = (row + lane) * 63 / (fade_rows - 1);
      const uint32_t left = 63 - reflected_row;
      alpha_values[lane] = (uint16_t)(150 * left * left / (63 * 63));
      threshold_values[lane] =
          (uint16_t)(reflection_bayer[reflected_row & 3][x & 3] * 16 + 8);
    }
    const uint16x8_t alpha = vld1q_u16(alpha_values);
    const uint16x8_t threshold = vld1q_u16(threshold_values);
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
    const size_t fade_row = row * 63 / (fade_rows - 1);
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

void magik_launcher_project_column(uint32_t *out, size_t pitch, size_t x,
                                   size_t top, size_t bottom,
                                   const uint32_t *src, size_t height,
                                   int32_t q, int32_t step) {
  size_t y = top;
  while (y < bottom) {
    int32_t row = q >> 16;
    int32_t next = q + step;
    int32_t row2 = next >> 16;
    if (y + 1 < bottom && row >= 0 && row2 >= 0 && (size_t)(row + 1) < height &&
        (size_t)(row2 + 1) < height) {
      // Gather two adjacent RGBA pairs, then interpolate all eight channels.
      // 16-bit weights preserve the exact scalar floor even when weight is 0.
      uint32x2x2_t samples =
          vtrn_u32(vld1_u32(src + row), vld1_u32(src + row2));
      uint16x8_t a = vmovl_u8(vreinterpret_u8_u32(samples.val[0]));
      uint16x8_t b = vmovl_u8(vreinterpret_u8_u32(samples.val[1]));
      uint16x8_t w = vcombine_u16(vdup_n_u16(((uint32_t)q & 65535) >> 8),
                                  vdup_n_u16(((uint32_t)next & 65535) >> 8));
      uint16x8_t value =
          vmlaq_u16(vmulq_u16(a, vsubq_u16(vdupq_n_u16(256), w)), b, w);
      uint32x2_t pixels = vreinterpret_u32_u8(vshrn_n_u16(value, 8));
      vst1_lane_u32(out + y * pitch + x, pixels, 0);
      vst1_lane_u32(out + (y + 1) * pitch + x, pixels, 1);
      y += 2;
      q = next + step;
      continue;
    }
    uint32_t a = row >= 0 && (size_t)row < height ? src[row] : 0;
    uint32_t b = row + 1 >= 0 && (size_t)(row + 1) < height ? src[row + 1] : 0;
    out[y * pitch + x] = scalar(a, b, ((uint32_t)q & 65535) >> 8);
    ++y;
    q = next;
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
void magik_launcher_over_row(uint16_t *out, const uint32_t *src, size_t n) {
  size_t i = 0;
  for (; i + 4 <= n; i += 4) {
    uint32x4_t p = vld1q_u32(src + i);
    uint32x4_t alpha = vshrq_n_u32(p, 24);
    uint32x2_t m = vmin_u32(vget_low_u32(alpha), vget_high_u32(alpha));
    if (vget_lane_u32(m, 0) == 255 && vget_lane_u32(m, 1) == 255) {
      uint32x4_t r = vshlq_n_u32(vandq_u32(p, vdupq_n_u32(248)), 8);
      uint32x4_t g =
          vshlq_n_u32(vandq_u32(vshrq_n_u32(p, 8), vdupq_n_u32(252)), 3);
      uint32x4_t b = vandq_u32(vshrq_n_u32(p, 19), vdupq_n_u32(31));
      vst1_u16(out + i, vmovn_u32(vorrq_u32(vorrq_u32(r, g), b)));
    } else {
      for (size_t j = 0; j < 4; ++j)
        out[i + j] = over_pixel(src[i + j], out[i + j]);
    }
  }
  for (; i < n; ++i)
    out[i] = over_pixel(src[i], out[i]);
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

void magik_launcher_flat_rgba(uint32_t *out, size_t pitch, const uint32_t *src,
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
        vst1q_u32(out + y * pitch + x, blend(vcombine_u32(ab.val[0], cd.val[0]),
                                            vcombine_u32(ab.val[1], cd.val[1]), w));
      }
    }
    for (; x < width; ++x) {
      uint32_t a = r >= 0 && (size_t)r < height ? src[x * stride + r] : 0;
      uint32_t b = r + 1 >= 0 && (size_t)(r + 1) < height ? src[x * stride + r + 1] : 0;
      out[y * pitch + x] = scalar(a, b, w);
    }
  }
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
