#include <arm_neon.h>
#include <stdint.h>

static inline uint8_t clamp_u8_i32(int value) {
    if (value < 0) {
        return 0;
    }
    if (value > 255) {
        return 255;
    }
    return (uint8_t)value;
}

static inline uint16_t i420_pixel_to_rgb565(uint8_t y, uint8_t u, uint8_t v) {
    int c = (int)y - 16;
    int d = (int)u - 128;
    int e = (int)v - 128;
    if (c < 0) {
        c = 0;
    }

    uint8_t r = clamp_u8_i32((298 * c + 409 * e + 128) >> 8);
    uint8_t g = clamp_u8_i32((298 * c - 100 * d - 208 * e + 128) >> 8);
    uint8_t b = clamp_u8_i32((298 * c + 516 * d + 128) >> 8);

    return (uint16_t)(((uint16_t)(r & 0xf8) << 8) |
                      ((uint16_t)(g & 0xfc) << 3) |
                      ((uint16_t)b >> 3));
}

void mister_i420_to_rgb565_neon(const uint8_t *src_y,
                                int src_stride_y,
                                const uint8_t *src_u,
                                int src_stride_u,
                                const uint8_t *src_v,
                                int src_stride_v,
                                uint16_t *dst_rgb565,
                                int dst_stride_rgb565,
                                int width,
                                int height) {
    for (int row = 0; row < height; row++) {
        const uint8_t *y_row = src_y + row * src_stride_y;
        const uint8_t *u_row = src_u + (row >> 1) * src_stride_u;
        const uint8_t *v_row = src_v + (row >> 1) * src_stride_v;
        uint16_t *dst_row = dst_rgb565 + row * dst_stride_rgb565;
        int x = 0;

        for (; x + 8 <= width; x += 8) {
            uint8x8_t y_u8 = vld1_u8(y_row + x);

            uint32_t u_word = (uint32_t)u_row[(x >> 1) + 0] |
                              ((uint32_t)u_row[(x >> 1) + 1] << 8) |
                              ((uint32_t)u_row[(x >> 1) + 2] << 16) |
                              ((uint32_t)u_row[(x >> 1) + 3] << 24);
            uint32_t v_word = (uint32_t)v_row[(x >> 1) + 0] |
                              ((uint32_t)v_row[(x >> 1) + 1] << 8) |
                              ((uint32_t)v_row[(x >> 1) + 2] << 16) |
                              ((uint32_t)v_row[(x >> 1) + 3] << 24);
            uint8x8_t u_dup = vzip_u8(vreinterpret_u8_u32(vdup_n_u32(u_word)),
                                      vreinterpret_u8_u32(vdup_n_u32(u_word)))
                                   .val[0];
            uint8x8_t v_dup = vzip_u8(vreinterpret_u8_u32(vdup_n_u32(v_word)),
                                      vreinterpret_u8_u32(vdup_n_u32(v_word)))
                                   .val[0];

            int16x8_t y = vreinterpretq_s16_u16(vmovl_u8(y_u8));
            int16x8_t u = vreinterpretq_s16_u16(vmovl_u8(u_dup));
            int16x8_t v = vreinterpretq_s16_u16(vmovl_u8(v_dup));
            y = vmaxq_s16(vsubq_s16(y, vdupq_n_s16(16)), vdupq_n_s16(0));
            u = vsubq_s16(u, vdupq_n_s16(128));
            v = vsubq_s16(v, vdupq_n_s16(128));

            /* Shift/add approximation trades tiny color error for the 60 Hz video budget. */
            int16x8_t y_base = vaddq_s16(y, vaddq_s16(vshrq_n_s16(y, 3), vshrq_n_s16(y, 5)));
            int16x8_t r16 = vaddq_s16(
                y_base,
                vaddq_s16(v, vaddq_s16(vshrq_n_s16(v, 2),
                                        vaddq_s16(vshrq_n_s16(v, 3), vshrq_n_s16(v, 5)))));
            int16x8_t g16 = vsubq_s16(
                vsubq_s16(y_base,
                          vaddq_s16(vshrq_n_s16(u, 2),
                                     vaddq_s16(vshrq_n_s16(u, 4), vshrq_n_s16(u, 5)))),
                vaddq_s16(vshrq_n_s16(v, 1),
                          vaddq_s16(vshrq_n_s16(v, 2), vshrq_n_s16(v, 5))));
            int16x8_t b16 = vaddq_s16(
                y_base,
                vaddq_s16(u, vaddq_s16(vshrq_n_s16(u, 1),
                                       vaddq_s16(vshrq_n_s16(u, 2), vshrq_n_s16(u, 4)))));

            uint8x8_t r = vqmovun_s16(r16);
            uint8x8_t g = vqmovun_s16(g16);
            uint8x8_t b = vqmovun_s16(b16);

            uint16x8_t r565 = vshlq_n_u16(vmovl_u8(vshr_n_u8(r, 3)), 11);
            uint16x8_t g565 = vshlq_n_u16(vmovl_u8(vshr_n_u8(g, 2)), 5);
            uint16x8_t b565 = vmovl_u8(vshr_n_u8(b, 3));
            uint16x8_t rgb565 = vorrq_u16(vorrq_u16(r565, g565), b565);
            vst1q_u16(dst_row + x, rgb565);
        }

        for (; x < width; x++) {
            dst_row[x] = i420_pixel_to_rgb565(y_row[x], u_row[x >> 1], v_row[x >> 1]);
        }
    }
}
