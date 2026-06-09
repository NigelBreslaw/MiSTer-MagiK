#include <stdint.h>
#include <arm_neon.h>

void neon_add_u8(const uint8_t *a, const uint8_t *b, uint8_t *out, uint32_t len) {
    uint32_t i = 0;
    for (; i + 16 <= len; i += 16) {
        uint8x16_t av = vld1q_u8(a + i);
        uint8x16_t bv = vld1q_u8(b + i);
        vst1q_u8(out + i, vaddq_u8(av, bv));
    }
    for (; i < len; i++) {
        out[i] = (uint8_t)(a[i] + b[i]);
    }
}

int32_t neon_dot_i16(const int16_t *a, const int16_t *b, uint32_t len) {
    uint32_t i = 0;
    int32x4_t acc = vdupq_n_s32(0);

    for (; i + 8 <= len; i += 8) {
        int16x8_t av = vld1q_s16(a + i);
        int16x8_t bv = vld1q_s16(b + i);
        acc = vmlal_s16(acc, vget_low_s16(av), vget_low_s16(bv));
        acc = vmlal_s16(acc, vget_high_s16(av), vget_high_s16(bv));
    }

    int32_t lanes[4] __attribute__((aligned(16)));
    vst1q_s32(lanes, acc);
    int32_t total = lanes[0] + lanes[1] + lanes[2] + lanes[3];

    for (; i < len; i++) {
        total += (int32_t)a[i] * (int32_t)b[i];
    }

    return total;
}
