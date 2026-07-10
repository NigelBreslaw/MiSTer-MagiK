#include <arm_neon.h>
#include <stddef.h>
#include <stdint.h>

#if __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "RGB565 NEON decimation requires the MiSTer little-endian pixel layout"
#endif

void mister_magik_downsample_rgb565_2x_scalar(const uint16_t *source,
                                               size_t source_height,
                                               size_t source_stride,
                                               uint16_t *destination,
                                               size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;

    for (size_t output_y = 0; output_y < destination_height; ++output_y) {
        const uint16_t *source_pixel = source + output_y * 2U * source_stride;
        uint16_t *destination_pixel = destination + output_y * destination_width;
        uint16_t *const destination_end = destination_pixel + destination_width;
        while (destination_pixel < destination_end) {
            *destination_pixel++ = *source_pixel;
            source_pixel += 2U;
        }
    }
}

void mister_magik_downsample_rgb565_2x_neon(const uint16_t *source,
                                             size_t source_width,
                                             size_t source_height,
                                             size_t source_stride,
                                             uint16_t *destination,
                                             size_t destination_width) {
    if (((uintptr_t)source & (sizeof(uint32_t) - 1U)) != 0U) {
        mister_magik_downsample_rgb565_2x_scalar(
            source, source_height, source_stride, destination, destination_width);
        return;
    }
    const size_t destination_height = (source_height + 1U) / 2U;

    for (size_t output_y = 0; output_y < destination_height; ++output_y) {
        const uint16_t *source_row = source + output_y * 2U * source_stride;
        uint16_t *destination_row = destination + output_y * destination_width;
        size_t output_x = 0;

        while (output_x + 32U <= destination_width &&
               output_x * 2U + 64U <= source_width) {
            const uint32_t *pairs =
                (const uint32_t *)(source_row + output_x * 2U);
            if (output_x + 64U < destination_width) {
                __builtin_prefetch(source_row + (output_x + 64U) * 2U, 0, 0);
            }
            const uint32x4_t a = vld1q_u32(pairs);
            const uint32x4_t b = vld1q_u32(pairs + 4U);
            const uint32x4_t c = vld1q_u32(pairs + 8U);
            const uint32x4_t d = vld1q_u32(pairs + 12U);
            const uint32x4_t e = vld1q_u32(pairs + 16U);
            const uint32x4_t f = vld1q_u32(pairs + 20U);
            const uint32x4_t g = vld1q_u32(pairs + 24U);
            const uint32x4_t h = vld1q_u32(pairs + 28U);
            vst1q_u16(destination_row + output_x,
                       vcombine_u16(vmovn_u32(a), vmovn_u32(b)));
            vst1q_u16(destination_row + output_x + 8U,
                       vcombine_u16(vmovn_u32(c), vmovn_u32(d)));
            vst1q_u16(destination_row + output_x + 16U,
                       vcombine_u16(vmovn_u32(e), vmovn_u32(f)));
            vst1q_u16(destination_row + output_x + 24U,
                       vcombine_u16(vmovn_u32(g), vmovn_u32(h)));
            output_x += 32U;
        }
        while (output_x + 16U <= destination_width &&
               output_x * 2U + 32U <= source_width) {
            const uint32_t *pairs = (const uint32_t *)(source_row + output_x * 2U);
            const uint32x4_t a = vld1q_u32(pairs);
            const uint32x4_t b = vld1q_u32(pairs + 4U);
            const uint32x4_t c = vld1q_u32(pairs + 8U);
            const uint32x4_t d = vld1q_u32(pairs + 12U);
            vst1q_u16(destination_row + output_x,
                       vcombine_u16(vmovn_u32(a), vmovn_u32(b)));
            vst1q_u16(destination_row + output_x + 8U,
                       vcombine_u16(vmovn_u32(c), vmovn_u32(d)));
            output_x += 16U;
        }
        while (output_x < destination_width) {
            destination_row[output_x] = source_row[output_x * 2U];
            ++output_x;
        }
    }
}
