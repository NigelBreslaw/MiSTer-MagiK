// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>

static const float DEPTH_EXTENT = 64.0f;
static const float DEPTH_FIXED_SCALE = 128.0f;
static const float DEPTH_FIXED_SCALE_RECIP = 1.0f / 128.0f;
static const float TARGET_FIXED_SCALE_RECIP = 1.0f / 16.0f;
static const float TARGET_DEPTH_Q2_RECIP = 1.0f / 4.0f;
static const float RANDOM_UNIT_RECIP = 1.0f / 16777215.0f;
static const float FOCAL_LENGTH = 720.0f;
static const float PROJECTION_CORRECTION_MARGIN = 0.01f;

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

static inline float32x4_t load_depth_q7(const int16_t *depths) {
    return vmulq_n_f32(
        vcvtq_f32_s32(vmovl_s16(vld1_s16(depths))),
        DEPTH_FIXED_SCALE_RECIP
    );
}

static inline void store_depth_q7(int16_t *depths, float32x4_t value) {
    float32x4_t scaled = vmulq_n_f32(clamp_depth(value), DEPTH_FIXED_SCALE);
    uint32x4_t negative = vcltq_f32(scaled, vdupq_n_f32(0.0f));
    float32x4_t adjustment = vbslq_f32(
        negative,
        vdupq_n_f32(-0.5f),
        vdupq_n_f32(0.5f)
    );
    vst1_s16(
        depths,
        vmovn_s32(vcvtq_s32_f32(vaddq_f32(scaled, adjustment)))
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
    int16_t *z_q7,
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
        store_depth_q7(
            z_q7 + index,
            vaddq_f32(
                load_depth_q7(z_q7 + index),
                vmulq_f32(next_vz, delta_vector)
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
    int16_t *z_q7,
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
        float32x4_t old_z = load_depth_q7(z_q7 + index);
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
        store_depth_q7(
            z_q7 + index,
            vaddq_f32(old_z, vmulq_f32(next_vz, delta_vector))
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
    int16_t *z_q7,
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
        float32x4_t old_z = load_depth_q7(z_q7 + index);
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
        store_depth_q7(
            z_q7 + index,
            vaddq_f32(old_z, vmulq_f32(next_vz, delta_vector))
        );
    }
    return vector_end;
}

static inline int projection_needs_correction(
    float screen_x,
    float screen_y,
    float projection_max_x,
    float projection_max_y
) {
    if (screen_x <= -0.5f + PROJECTION_CORRECTION_MARGIN ||
        screen_y <= -0.5f + PROJECTION_CORRECTION_MARGIN ||
        screen_x >= projection_max_x - PROJECTION_CORRECTION_MARGIN ||
        screen_y >= projection_max_y - PROJECTION_CORRECTION_MARGIN) {
        return 1;
    }
    float shifted_x = screen_x + 0.5f;
    float shifted_y = screen_y + 0.5f;
    float fraction_x = shifted_x - (float)(int32_t)shifted_x;
    float fraction_y = shifted_y - (float)(int32_t)shifted_y;
    return fraction_x < PROJECTION_CORRECTION_MARGIN ||
           fraction_x > 1.0f - PROJECTION_CORRECTION_MARGIN ||
           fraction_y < PROJECTION_CORRECTION_MARGIN ||
           fraction_y > 1.0f - PROJECTION_CORRECTION_MARGIN;
}

size_t mister_magik_particle_neon_project_offsets(
    size_t count,
    size_t width,
    float projection_center_x,
    float projection_center_y,
    float projection_max_x,
    float projection_max_y,
    float rotation_y_sin,
    float rotation_y_cos,
    const float *x,
    const float *y,
    const int16_t *z_q7,
    uint32_t *offsets
) {
    size_t visible = 0;
    size_t vector_end = count & ~(size_t)3;
    float32x4_t center_x = vdupq_n_f32(projection_center_x);
    float32x4_t center_y = vdupq_n_f32(projection_center_y);
    float32x4_t focal = vdupq_n_f32(FOCAL_LENGTH);
    for (size_t index = 0; index < vector_end; index += 4) {
        float32x4_t relative_x = vsubq_f32(vld1q_f32(x + index), center_x);
        float32x4_t source_z = load_depth_q7(z_q7 + index);
        float32x4_t rotated_x = vaddq_f32(
            vmulq_n_f32(relative_x, rotation_y_cos),
            vmulq_n_f32(source_z, rotation_y_sin)
        );
        float32x4_t rotated_z = vaddq_f32(
            vmulq_n_f32(relative_x, -rotation_y_sin),
            vmulq_n_f32(source_z, rotation_y_cos)
        );
        float32x4_t denominator = vaddq_f32(focal, rotated_z);
        float32x4_t reciprocal = vrecpeq_f32(denominator);
        reciprocal = vmulq_f32(
            reciprocal,
            vrecpsq_f32(denominator, reciprocal)
        );
        reciprocal = vmulq_f32(
            reciprocal,
            vrecpsq_f32(denominator, reciprocal)
        );
        float32x4_t scale = vmulq_f32(focal, reciprocal);
        float32x4_t screen_x = vaddq_f32(
            center_x,
            vmulq_f32(rotated_x, scale)
        );
        float32x4_t screen_y = vaddq_f32(
            center_y,
            vmulq_f32(vsubq_f32(vld1q_f32(y + index), center_y), scale)
        );
        float denominator_values[4];
        float rotated_x_values[4];
        float rotated_z_values[4];
        float screen_x_values[4];
        float screen_y_values[4];
        vst1q_f32(denominator_values, denominator);
        vst1q_f32(rotated_x_values, rotated_x);
        vst1q_f32(rotated_z_values, rotated_z);
        vst1q_f32(screen_x_values, screen_x);
        vst1q_f32(screen_y_values, screen_y);
        for (size_t lane = 0; lane < 4; lane++) {
            float lane_x = screen_x_values[lane];
            float lane_y = screen_y_values[lane];
            if (denominator_values[lane] <= 1.0f) {
                offsets[index + lane] = UINT32_MAX;
                continue;
            }
            if (projection_needs_correction(
                    lane_x,
                    lane_y,
                    projection_max_x,
                    projection_max_y)) {
                float exact_scale = FOCAL_LENGTH / denominator_values[lane];
                lane_x =
                    projection_center_x + rotated_x_values[lane] * exact_scale;
                lane_y = projection_center_y +
                    (y[index + lane] - projection_center_y) * exact_scale;
            }
            if (lane_x <= -0.5f || lane_y <= -0.5f ||
                lane_x >= projection_max_x || lane_y >= projection_max_y) {
                offsets[index + lane] = UINT32_MAX;
                continue;
            }
            uint32_t pixel_x = (uint32_t)(lane_x + 0.5f);
            uint32_t pixel_y = (uint32_t)(lane_y + 0.5f);
            offsets[index + lane] =
                pixel_y * (uint32_t)width + pixel_x;
            visible++;
        }
    }
    for (size_t index = vector_end; index < count; index++) {
        float relative_x = x[index] - projection_center_x;
        float source_z = (float)z_q7[index] * DEPTH_FIXED_SCALE_RECIP;
        float rotated_x =
            relative_x * rotation_y_cos + source_z * rotation_y_sin;
        float rotated_z =
            -relative_x * rotation_y_sin + source_z * rotation_y_cos;
        float denominator = FOCAL_LENGTH + rotated_z;
        if (denominator <= 1.0f) {
            offsets[index] = UINT32_MAX;
            continue;
        }
        float scale = FOCAL_LENGTH / denominator;
        float screen_x = projection_center_x + rotated_x * scale;
        float screen_y = projection_center_y +
            (y[index] - projection_center_y) * scale;
        if (screen_x <= -0.5f || screen_y <= -0.5f ||
            screen_x >= projection_max_x || screen_y >= projection_max_y) {
            offsets[index] = UINT32_MAX;
            continue;
        }
        uint32_t pixel_x = (uint32_t)(screen_x + 0.5f);
        uint32_t pixel_y = (uint32_t)(screen_y + 0.5f);
        offsets[index] = pixel_y * (uint32_t)width + pixel_x;
        visible++;
    }
    return visible;
}
