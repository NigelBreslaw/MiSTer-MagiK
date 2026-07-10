#include <stddef.h>
#include <stdint.h>

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
