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
        const uint16_t *source_row = source + output_y * 2U * source_stride;
        uint16_t *destination_row = destination + output_y * destination_width;
        for (size_t output_x = 0; output_x < destination_width; ++output_x) {
            destination_row[output_x] = source_row[output_x * 2U];
        }
    }
}

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

        while (output_x + 8U <= destination_width &&
               output_x * 2U + 16U <= source_width) {
            const uint32_t *pairs = (const uint32_t *)(source_row + output_x * 2U);
            const uint32x4_t first = vld1q_u32(pairs);
            const uint32x4_t second = vld1q_u32(pairs + 4U);
            const uint16x8_t selected =
                vcombine_u16(vmovn_u32(first), vmovn_u32(second));
            vst1q_u16(destination_row + output_x, selected);
            output_x += 8U;
        }
        while (output_x < destination_width) {
            destination_row[output_x] = source_row[output_x * 2U];
            ++output_x;
        }
    }
}
