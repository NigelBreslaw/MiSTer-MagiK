// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>

static const float DEPTH_EXTENT = 64.0f;
static const float TARGET_FIXED_SCALE_RECIP = 1.0f / 16.0f;
static const float TARGET_DEPTH_Q2_RECIP = 1.0f / 4.0f;
static const float RANDOM_UNIT_RECIP = 1.0f / 16777215.0f;

static inline uint32x4_t next_random(uint32_t *states) {
    uint32x4_t value = vld1q_u32(states);
    value = veorq_u32(value, vshlq_n_u32(value, 13));
    value = veorq_u32(value, vshrq_n_u32(value, 17));
    value = veorq_u32(value, vshlq_n_u32(value, 5));
    vst1q_u32(states, value);
    return value;
}

static inline uint32x4_t rotate_left_11(uint32x4_t value) {
    return vorrq_u32(vshlq_n_u32(value, 11), vshrq_n_u32(value, 21));
}

static inline uint32x4_t rotate_left_21(uint32x4_t value) {
    return vorrq_u32(vshlq_n_u32(value, 21), vshrq_n_u32(value, 11));
}

static inline float32x4_t signed_unit_vector(uint32x4_t value) {
    float32x4_t unit =
        vmulq_n_f32(vcvtq_f32_u32(vshrq_n_u32(value, 8)), RANDOM_UNIT_RECIP);
    return vsubq_f32(vmulq_n_f32(unit, 2.0f), vdupq_n_f32(1.0f));
}

static inline void target_xy(
    const uint32_t *targets,
    float32x4_t *target_x,
    float32x4_t *target_y
) {
    int32x4_t packed = vreinterpretq_s32_u32(vld1q_u32(targets));
    int32x4_t x = vshrq_n_s32(vshlq_n_s32(packed, 16), 16);
    int32x4_t y = vshrq_n_s32(packed, 16);
    *target_x = vmulq_n_f32(vcvtq_f32_s32(x), TARGET_FIXED_SCALE_RECIP);
    *target_y = vmulq_n_f32(vcvtq_f32_s32(y), TARGET_FIXED_SCALE_RECIP);
}

static inline float32x4_t target_z(const int8_t *depths) {
    int32_t values[4] = {depths[0], depths[1], depths[2], depths[3]};
    return vmulq_n_f32(vcvtq_f32_s32(vld1q_s32(values)), TARGET_DEPTH_Q2_RECIP);
}

static inline float32x4_t clamp_depth(float32x4_t value) {
    return vminq_f32(
        vmaxq_f32(value, vdupq_n_f32(-DEPTH_EXTENT)),
        vdupq_n_f32(DEPTH_EXTENT)
    );
}

static inline float32x4_t wrap_once(float32x4_t value, float extent_value) {
    float32x4_t zero = vdupq_n_f32(0.0f);
    float32x4_t extent = vdupq_n_f32(extent_value);
    float32x4_t added =
        vbslq_f32(vcltq_f32(value, zero), vaddq_f32(value, extent), value);
    return vbslq_f32(
        vcgeq_f32(added, extent),
        vsubq_f32(added, extent),
        added
    );
}

static inline float wrap_coordinate(float value, float extent) {
    if (value >= 0.0f && value < extent) {
        return value;
    }
    float wrapped = fmodf(value, extent);
    return wrapped < 0.0f ? wrapped + extent : wrapped;
}

size_t mister_magik_particle_neon_static(
    size_t count,
    float width,
    float height,
    float delta,
    uint32_t *random_states,
    float *x,
    float *y,
    float *z,
    float *vx,
    float *vy,
    float *vz
) {
    size_t vector_end = count & ~(size_t)3;
    float32x4_t delta_vector = vdupq_n_f32(delta);
    for (size_t index = 0; index < vector_end; index += 4) {
        uint32x4_t noise = next_random(random_states + index);
        float32x4_t jitter_x = signed_unit_vector(noise);
        float32x4_t jitter_y = signed_unit_vector(rotate_left_11(noise));
        float32x4_t jitter_z = signed_unit_vector(rotate_left_21(noise));
        float32x4_t force_x =
            vmulq_n_f32(vmulq_n_f32(jitter_x, 75.0f), delta);
        float32x4_t force_y =
            vmulq_n_f32(vmulq_n_f32(jitter_y, 75.0f), delta);
        float32x4_t force_z =
            vmulq_n_f32(vmulq_n_f32(jitter_z, 8.0f), delta);
        float32x4_t next_vx =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vx + index), force_x), 0.985f);
        float32x4_t next_vy =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vy + index), force_y), 0.985f);
        float32x4_t next_vz =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vz + index), force_z), 0.98f);
        vst1q_f32(vx + index, next_vx);
        vst1q_f32(vy + index, next_vy);
        vst1q_f32(vz + index, next_vz);
        vst1q_f32(
            x + index,
            wrap_once(
                vaddq_f32(vld1q_f32(x + index), vmulq_f32(next_vx, delta_vector)),
                width
            )
        );
        vst1q_f32(
            y + index,
            wrap_once(
                vaddq_f32(vld1q_f32(y + index), vmulq_f32(next_vy, delta_vector)),
                height
            )
        );
        vst1q_f32(
            z + index,
            clamp_depth(
                vaddq_f32(vld1q_f32(z + index), vmulq_f32(next_vz, delta_vector))
            )
        );
        for (size_t lane = index; lane < index + 4; lane++) {
            if (x[lane] < 0.0f || x[lane] >= width) {
                x[lane] = wrap_coordinate(x[lane], width);
            }
            if (y[lane] < 0.0f || y[lane] >= height) {
                y[lane] = wrap_coordinate(y[lane], height);
            }
        }
    }
    return vector_end;
}

size_t mister_magik_particle_neon_attract(
    size_t count,
    float delta,
    float stiffness,
    float jitter,
    float damping,
    const uint32_t *packed_targets,
    const int8_t *target_depth_q2,
    uint32_t *random_states,
    float *x,
    float *y,
    float *z,
    float *vx,
    float *vy,
    float *vz
) {
    size_t vector_end = count & ~(size_t)3;
    float32x4_t delta_vector = vdupq_n_f32(delta);
    for (size_t index = 0; index < vector_end; index += 4) {
        uint32x4_t noise = next_random(random_states + index);
        float32x4_t jitter_x = signed_unit_vector(noise);
        float32x4_t jitter_y = signed_unit_vector(rotate_left_11(noise));
        float32x4_t target_x;
        float32x4_t target_y;
        target_xy(packed_targets + index, &target_x, &target_y);
        float32x4_t target_depth = target_z(target_depth_q2 + index);
        float32x4_t old_x = vld1q_f32(x + index);
        float32x4_t old_y = vld1q_f32(y + index);
        float32x4_t old_z = vld1q_f32(z + index);
        float32x4_t force_x = vmulq_n_f32(
            vmulq_n_f32(
                vsubq_f32(
                    vaddq_f32(target_x, vmulq_n_f32(jitter_x, jitter)),
                    old_x
                ),
                stiffness
            ),
            delta
        );
        float32x4_t force_y = vmulq_n_f32(
            vmulq_n_f32(
                vsubq_f32(
                    vaddq_f32(target_y, vmulq_n_f32(jitter_y, jitter)),
                    old_y
                ),
                stiffness
            ),
            delta
        );
        float32x4_t force_z = vmulq_n_f32(
            vmulq_n_f32(vsubq_f32(target_depth, old_z), stiffness),
            delta
        );
        float32x4_t next_vx =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vx + index), force_x), damping);
        float32x4_t next_vy =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vy + index), force_y), damping);
        float32x4_t next_vz =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vz + index), force_z), damping);
        vst1q_f32(vx + index, next_vx);
        vst1q_f32(vy + index, next_vy);
        vst1q_f32(vz + index, next_vz);
        vst1q_f32(x + index, vaddq_f32(old_x, vmulq_f32(next_vx, delta_vector)));
        vst1q_f32(y + index, vaddq_f32(old_y, vmulq_f32(next_vy, delta_vector)));
        vst1q_f32(
            z + index,
            clamp_depth(vaddq_f32(old_z, vmulq_f32(next_vz, delta_vector)))
        );
    }
    return vector_end;
}

size_t mister_magik_particle_neon_disperse(
    size_t count,
    float delta,
    const uint32_t *packed_targets,
    uint32_t *random_states,
    float *x,
    float *y,
    float *z,
    float *vx,
    float *vy,
    float *vz
) {
    size_t vector_end = count & ~(size_t)3;
    float32x4_t delta_vector = vdupq_n_f32(delta);
    for (size_t index = 0; index < vector_end; index += 4) {
        uint32x4_t noise = next_random(random_states + index);
        float32x4_t jitter_x = signed_unit_vector(noise);
        float32x4_t jitter_y = signed_unit_vector(rotate_left_11(noise));
        float32x4_t jitter_z = signed_unit_vector(rotate_left_21(noise));
        float32x4_t target_x;
        float32x4_t target_y;
        target_xy(packed_targets + index, &target_x, &target_y);
        float32x4_t old_x = vld1q_f32(x + index);
        float32x4_t old_y = vld1q_f32(y + index);
        float32x4_t old_z = vld1q_f32(z + index);
        float32x4_t force_x = vmulq_n_f32(
            vaddq_f32(
                vmulq_n_f32(vsubq_f32(old_x, target_x), 2.2f),
                vmulq_n_f32(jitter_x, 115.0f)
            ),
            delta
        );
        float32x4_t force_y = vmulq_n_f32(
            vaddq_f32(
                vmulq_n_f32(vsubq_f32(old_y, target_y), 2.2f),
                vmulq_n_f32(jitter_y, 115.0f)
            ),
            delta
        );
        float32x4_t force_z =
            vmulq_n_f32(vmulq_n_f32(jitter_z, 55.0f), delta);
        float32x4_t next_vx =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vx + index), force_x), 0.99f);
        float32x4_t next_vy =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vy + index), force_y), 0.99f);
        float32x4_t next_vz =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vz + index), force_z), 0.99f);
        vst1q_f32(vx + index, next_vx);
        vst1q_f32(vy + index, next_vy);
        vst1q_f32(vz + index, next_vz);
        vst1q_f32(x + index, vaddq_f32(old_x, vmulq_f32(next_vx, delta_vector)));
        vst1q_f32(y + index, vaddq_f32(old_y, vmulq_f32(next_vy, delta_vector)));
        vst1q_f32(
            z + index,
            clamp_depth(vaddq_f32(old_z, vmulq_f32(next_vz, delta_vector)))
        );
    }
    return vector_end;
}
