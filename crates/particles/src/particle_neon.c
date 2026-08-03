// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#include <arm_neon.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

static const float DEPTH_FIXED_SCALE = 128.0f;
static const float DEPTH_FIXED_SCALE_RECIP = 1.0f / 128.0f;
static const float TARGET_FIXED_SCALE_RECIP = 1.0f / 16.0f;
static const float TARGET_DEPTH_Q2_RECIP = 1.0f / 4.0f;
static const float RANDOM_UNIT_RECIP = 1.0f / 16777215.0f;
static const uint32_t PARTICLE_NOT_VISIBLE = UINT32_MAX;
static const uint32_t COMMAND_PALETTE_SHIFT = 20;
static const uint32_t COMMAND_NEIGHBOR = 1u << 22;

typedef struct {
    float target_x[4];
    float target_y[4];
    float target_z[4];
    float source_x[4];
    float source_y[4];
    float source_z[4];
} cabinet_position_block;

size_t mister_magik_cabinet_neon_project_stable(
    size_t count,
    const cabinet_position_block *restrict blocks,
    size_t first_block,
    size_t block_step,
    float sin_yaw,
    float cos_yaw,
    float sin_pitch,
    float cos_pitch,
    float dolly,
    float near_depth,
    float focal_length,
    float center_x,
    float center_y,
    uint32_t width,
    uint32_t height,
    uint32_t *restrict offsets
) {
    const size_t vector_end = count & ~(size_t)3;
    const size_t block_end = vector_end / 4;
    const float32x4_t sin_yaw_v = vdupq_n_f32(sin_yaw);
    const float32x4_t cos_yaw_v = vdupq_n_f32(cos_yaw);
    const float32x4_t sin_pitch_v = vdupq_n_f32(sin_pitch);
    const float32x4_t cos_pitch_v = vdupq_n_f32(cos_pitch);
    const float32x4_t dolly_v = vdupq_n_f32(dolly);
    const float32x4_t near_v = vdupq_n_f32(near_depth);
    const float32x4_t focal_v = vdupq_n_f32(focal_length);
    const float32x4_t center_x_v = vdupq_n_f32(center_x);
    const float32x4_t center_y_v = vdupq_n_f32(center_y);
    const float32x4_t zero = vdupq_n_f32(0.0f);
    const float32x4_t width_f = vdupq_n_f32((float)width);
    const float32x4_t height_f = vdupq_n_f32((float)height);
    const uint32x4_t width_u = vdupq_n_u32(width);
    const uint32x4_t invalid = vdupq_n_u32(UINT32_MAX);
    const float32x4_t depth_band_1 = vdupq_n_f32(480.0f);
    const float32x4_t depth_band_2 = vdupq_n_f32(640.0f);
    const float32x4_t depth_band_3 = vdupq_n_f32(800.0f);
    for (size_t block_index = first_block; block_index < block_end; block_index += block_step) {
        const size_t index = block_index * 4;
        const cabinet_position_block *block = blocks + block_index;
        const float32x4_t world_x = vld1q_f32(block->target_x);
        const float32x4_t world_y = vld1q_f32(block->target_y);
        const float32x4_t world_z = vld1q_f32(block->target_z);
        const float32x4_t rotated_x = vaddq_f32(
            vmulq_f32(world_x, cos_yaw_v),
            vmulq_f32(world_z, sin_yaw_v)
        );
        const float32x4_t yaw_z = vsubq_f32(
            vmulq_f32(world_z, cos_yaw_v),
            vmulq_f32(world_x, sin_yaw_v)
        );
        const float32x4_t rotated_y = vsubq_f32(
            vmulq_f32(world_y, cos_pitch_v),
            vmulq_f32(yaw_z, sin_pitch_v)
        );
        const float32x4_t rotated_z = vaddq_f32(
            vmulq_f32(world_y, sin_pitch_v),
            vmulq_f32(yaw_z, cos_pitch_v)
        );
        const float32x4_t depth = vaddq_f32(dolly_v, rotated_z);
        const uint32x4_t depth_valid = vcgtq_f32(depth, near_v);
        const float32x4_t safe_depth = vmaxq_f32(depth, near_v);
        float32x4_t reciprocal = vrecpeq_f32(safe_depth);
        reciprocal = vmulq_f32(reciprocal, vrecpsq_f32(safe_depth, reciprocal));
        reciprocal = vmulq_f32(reciprocal, vrecpsq_f32(safe_depth, reciprocal));
        const float32x4_t scale = vmulq_f32(focal_v, reciprocal);
        const float32x4_t x = vaddq_f32(center_x_v, vmulq_f32(rotated_x, scale));
        const float32x4_t y = vaddq_f32(center_y_v, vmulq_f32(rotated_y, scale));
        uint32x4_t valid = vandq_u32(depth_valid, vcgeq_f32(x, zero));
        valid = vandq_u32(valid, vcgeq_f32(y, zero));
        valid = vandq_u32(valid, vcltq_f32(x, width_f));
        valid = vandq_u32(valid, vcltq_f32(y, height_f));
        const uint32x4_t pixel_x = vcvtq_u32_f32(x);
        const uint32x4_t pixel_y = vcvtq_u32_f32(y);
        const uint32x4_t offset = vmlaq_u32(pixel_x, pixel_y, width_u);
        uint32x4_t depth_band = vshrq_n_u32(vcgeq_f32(depth, depth_band_1), 31);
        depth_band = vaddq_u32(
            depth_band,
            vshrq_n_u32(vcgeq_f32(depth, depth_band_2), 31)
        );
        depth_band = vaddq_u32(
            depth_band,
            vshrq_n_u32(vcgeq_f32(depth, depth_band_3), 31)
        );
        const uint32x4_t command = vorrq_u32(
            offset,
            vshlq_n_u32(depth_band, COMMAND_PALETTE_SHIFT)
        );
        vst1q_u32(offsets + index, vbslq_u32(valid, command, invalid));
    }
    return vector_end;
}

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
    int32_t packed;
    memcpy(&packed, depths, sizeof(packed));
    int8x8_t bytes = vreinterpret_s8_s32(vdup_n_s32(packed));
    int16x8_t wide16 = vmovl_s8(bytes);
    int32x4_t wide32 = vmovl_s16(vget_low_s16(wide16));
    return vmulq_n_f32(vcvtq_f32_s32(wide32), TARGET_DEPTH_Q2_RECIP);
}

static inline float32x4_t clamp_depth(float32x4_t value, float depth_extent) {
    return vminq_f32(
        vmaxq_f32(value, vdupq_n_f32(-depth_extent)),
        vdupq_n_f32(depth_extent)
    );
}

static inline float32x4_t load_depth_q7(const int16_t *depths) {
    return vmulq_n_f32(
        vcvtq_f32_s32(vmovl_s16(vld1_s16(depths))),
        DEPTH_FIXED_SCALE_RECIP
    );
}

static inline void store_depth_q7(
    int16_t *depths,
    float32x4_t value,
    float depth_extent
) {
    float32x4_t scaled = vmulq_n_f32(
        clamp_depth(value, depth_extent),
        DEPTH_FIXED_SCALE
    );
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
    float depth_extent,
    float acceleration_xy,
    float acceleration_z,
    float damping_xy,
    float damping_z,
    uint32_t *restrict random_states,
    float *restrict x,
    float *restrict y,
    int16_t *restrict z_q7,
    float *restrict vx,
    float *restrict vy,
    float *restrict vz
) {
    size_t vector_end = count & ~(size_t)3;
    float32x4_t delta_vector = vdupq_n_f32(delta);
    float jitter_xy_gain = acceleration_xy * delta;
    float jitter_z_gain = acceleration_z * delta;
    for (size_t index = 0; index < vector_end; index += 4) {
        uint32x4_t noise = next_random(random_states + index);
        float32x4_t jitter_x = signed_unit_vector(noise);
        float32x4_t jitter_y = signed_unit_vector(rotate_left_11(noise));
        float32x4_t jitter_z = signed_unit_vector(rotate_left_21(noise));
        float32x4_t force_x = vmulq_n_f32(jitter_x, jitter_xy_gain);
        float32x4_t force_y = vmulq_n_f32(jitter_y, jitter_xy_gain);
        float32x4_t force_z = vmulq_n_f32(jitter_z, jitter_z_gain);
        float32x4_t next_vx =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vx + index), force_x), damping_xy);
        float32x4_t next_vy =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vy + index), force_y), damping_xy);
        float32x4_t next_vz =
            vmulq_n_f32(vaddq_f32(vld1q_f32(vz + index), force_z), damping_z);
        vst1q_f32(vx + index, next_vx);
        vst1q_f32(vy + index, next_vy);
        vst1q_f32(vz + index, next_vz);
        float32x4_t next_x = wrap_once(
            vaddq_f32(vld1q_f32(x + index), vmulq_f32(next_vx, delta_vector)),
            width
        );
        float32x4_t next_y = wrap_once(
            vaddq_f32(vld1q_f32(y + index), vmulq_f32(next_vy, delta_vector)),
            height
        );
        vst1q_f32(x + index, next_x);
        vst1q_f32(y + index, next_y);
        store_depth_q7(
            z_q7 + index,
            vaddq_f32(
                load_depth_q7(z_q7 + index),
                vmulq_f32(next_vz, delta_vector)
            ),
            depth_extent
        );
        uint32x4_t exceptional = vorrq_u32(
            vorrq_u32(
                vcltq_f32(next_x, vdupq_n_f32(0.0f)),
                vcgeq_f32(next_x, vdupq_n_f32(width))
            ),
            vorrq_u32(
                vcltq_f32(next_y, vdupq_n_f32(0.0f)),
                vcgeq_f32(next_y, vdupq_n_f32(height))
            )
        );
        uint64x2_t exceptional_pairs = vreinterpretq_u64_u32(exceptional);
        if ((vgetq_lane_u64(exceptional_pairs, 0) |
             vgetq_lane_u64(exceptional_pairs, 1)) != 0) {
            for (size_t lane = index; lane < index + 4; lane++) {
                if (x[lane] < 0.0f || x[lane] >= width) {
                    x[lane] = wrap_coordinate(x[lane], width);
                }
                if (y[lane] < 0.0f || y[lane] >= height) {
                    y[lane] = wrap_coordinate(y[lane], height);
                }
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
    float depth_extent,
    const uint32_t *restrict packed_targets,
    const int8_t *restrict target_depth_q2,
    uint32_t *restrict random_states,
    float *restrict x,
    float *restrict y,
    int16_t *restrict z_q7,
    float *restrict vx,
    float *restrict vy,
    float *restrict vz
) {
    size_t vector_end = count & ~(size_t)3;
    float32x4_t delta_vector = vdupq_n_f32(delta);
    float force_gain = stiffness * delta;
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
            vsubq_f32(
                vaddq_f32(target_x, vmulq_n_f32(jitter_x, jitter)),
                old_x
            ),
            force_gain
        );
        float32x4_t force_y = vmulq_n_f32(
            vsubq_f32(
                vaddq_f32(target_y, vmulq_n_f32(jitter_y, jitter)),
                old_y
            ),
            force_gain
        );
        float32x4_t force_z = vmulq_n_f32(
            vsubq_f32(target_depth, old_z),
            force_gain
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
            vaddq_f32(old_z, vmulq_f32(next_vz, delta_vector)),
            depth_extent
        );
    }
    return vector_end;
}

size_t mister_magik_particle_neon_disperse(
    size_t count,
    float delta,
    float depth_extent,
    float outward_acceleration,
    float jitter_xy_acceleration,
    float jitter_z_acceleration,
    float damping,
    const uint32_t *restrict packed_targets,
    uint32_t *restrict random_states,
    float *restrict x,
    float *restrict y,
    int16_t *restrict z_q7,
    float *restrict vx,
    float *restrict vy,
    float *restrict vz
) {
    size_t vector_end = count & ~(size_t)3;
    float32x4_t delta_vector = vdupq_n_f32(delta);
    float repulsion_gain = outward_acceleration * delta;
    float jitter_xy_gain = jitter_xy_acceleration * delta;
    float jitter_z_gain = jitter_z_acceleration * delta;
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
            vsubq_f32(old_x, target_x),
            repulsion_gain
        );
        force_x = vaddq_f32(force_x, vmulq_n_f32(jitter_x, jitter_xy_gain));
        float32x4_t force_y = vmulq_n_f32(
            vsubq_f32(old_y, target_y),
            repulsion_gain
        );
        force_y = vaddq_f32(force_y, vmulq_n_f32(jitter_y, jitter_xy_gain));
        float32x4_t force_z = vmulq_n_f32(jitter_z, jitter_z_gain);
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
            vaddq_f32(old_z, vmulq_f32(next_vz, delta_vector)),
            depth_extent
        );
    }
    return vector_end;
}

static inline float32x4_t reciprocal_once(float32x4_t denominator) {
    float32x4_t reciprocal = vrecpeq_f32(denominator);
    return vmulq_f32(
        reciprocal,
        vrecpsq_f32(denominator, reciprocal)
    );
}

static inline uint32x4_t project_four_commands(
    size_t index,
    uint32_t width,
    uint32_t visual,
    uint32_t formed,
    float focal_length,
    float near_denominator,
    float formed_neighbor_when_depth_below,
    uint32_t unformed_palette_index,
    float32x4_t center_x,
    float32x4_t center_y,
    float32x4_t max_x,
    float32x4_t max_y,
    float32x4_t sin_y,
    float32x4_t cos_y,
    const float *x,
    const float *y,
    const int16_t *z_q7,
    const uint32_t *random_states,
    uint32x4_t *visible_count
) {
    float32x4_t relative_x = vsubq_f32(vld1q_f32(x + index), center_x);
    float32x4_t source_z = load_depth_q7(z_q7 + index);
    float32x4_t rotated_x = vaddq_f32(
        vmulq_f32(relative_x, cos_y),
        vmulq_f32(source_z, sin_y)
    );
    float32x4_t rotated_z = vsubq_f32(
        vmulq_f32(source_z, cos_y),
        vmulq_f32(relative_x, sin_y)
    );
    float32x4_t denominator =
        vaddq_f32(vdupq_n_f32(focal_length), rotated_z);
    float32x4_t scale =
        vmulq_n_f32(reciprocal_once(denominator), focal_length);
    float32x4_t screen_x =
        vaddq_f32(center_x, vmulq_f32(rotated_x, scale));
    float32x4_t screen_y = vaddq_f32(
        center_y,
        vmulq_f32(vsubq_f32(vld1q_f32(y + index), center_y), scale)
    );
    uint32x4_t visible_mask =
        vcgtq_f32(denominator, vdupq_n_f32(near_denominator));
    visible_mask = vandq_u32(
        visible_mask,
        vcgtq_f32(screen_x, vdupq_n_f32(-0.5f))
    );
    visible_mask = vandq_u32(
        visible_mask,
        vcgtq_f32(screen_y, vdupq_n_f32(-0.5f))
    );
    visible_mask = vandq_u32(visible_mask, vcltq_f32(screen_x, max_x));
    visible_mask = vandq_u32(visible_mask, vcltq_f32(screen_y, max_y));

    uint32x4_t pixel_x =
        vcvtq_u32_f32(vaddq_f32(screen_x, vdupq_n_f32(0.5f)));
    uint32x4_t pixel_y =
        vcvtq_u32_f32(vaddq_f32(screen_y, vdupq_n_f32(0.5f)));
    uint32x4_t command = vmlaq_n_u32(pixel_x, pixel_y, width);
    if (visual != 0) {
        uint32x4_t palette =
            vshrq_n_u32(vld1q_u32(random_states + index), 30);
        command = vorrq_u32(
            command,
            vshlq_n_u32(palette, COMMAND_PALETTE_SHIFT)
        );
        uint32x4_t neighbor_mask = vcltq_u32(
            pixel_x,
            vdupq_n_u32(width - 1)
        );
        uint32x4_t phase_neighbor = formed != 0
            ? vcltq_f32(
                rotated_z,
                vdupq_n_f32(formed_neighbor_when_depth_below)
            )
            : vceqq_u32(palette, vdupq_n_u32(unformed_palette_index));
        neighbor_mask = vandq_u32(
            visible_mask,
            vandq_u32(neighbor_mask, phase_neighbor)
        );
        command = vorrq_u32(
            command,
            vandq_u32(neighbor_mask, vdupq_n_u32(COMMAND_NEIGHBOR))
        );
    }
    *visible_count = vaddq_u32(
        *visible_count,
        vshrq_n_u32(visible_mask, 31)
    );
    return vbslq_u32(
        visible_mask,
        command,
        vdupq_n_u32(PARTICLE_NOT_VISIBLE)
    );
}

size_t mister_magik_particle_neon_project_commands(
    size_t count,
    size_t width,
    uint32_t visual,
    uint32_t phase,
    float focal_length,
    float near_denominator,
    float formed_neighbor_when_depth_below,
    uint32_t unformed_palette_index,
    float projection_center_x,
    float projection_center_y,
    float projection_max_x,
    float projection_max_y,
    float rotation_y_sin,
    float rotation_y_cos,
    const float *restrict x,
    const float *restrict y,
    const int16_t *restrict z_q7,
    const uint32_t *restrict random_states,
    uint32_t *restrict commands
) {
    size_t vector_end = count & ~(size_t)7;
    uint32x4_t visible_count = vdupq_n_u32(0);
    float32x4_t center_x = vdupq_n_f32(projection_center_x);
    float32x4_t center_y = vdupq_n_f32(projection_center_y);
    float32x4_t max_x = vdupq_n_f32(projection_max_x);
    float32x4_t max_y = vdupq_n_f32(projection_max_y);
    float32x4_t sin_y = vdupq_n_f32(rotation_y_sin);
    float32x4_t cos_y = vdupq_n_f32(rotation_y_cos);
    uint32_t formed = phase == 1 || phase == 2;
    uint32_t width_u32 = (uint32_t)width;
    for (size_t index = 0; index < vector_end; index += 8) {
        vst1q_u32(
            commands + index,
            project_four_commands(
                index,
                width_u32,
                visual,
                formed,
                focal_length,
                near_denominator,
                formed_neighbor_when_depth_below,
                unformed_palette_index,
                center_x,
                center_y,
                max_x,
                max_y,
                sin_y,
                cos_y,
                x,
                y,
                z_q7,
                random_states,
                &visible_count
            )
        );
        vst1q_u32(
            commands + index + 4,
            project_four_commands(
                index + 4,
                width_u32,
                visual,
                formed,
                focal_length,
                near_denominator,
                formed_neighbor_when_depth_below,
                unformed_palette_index,
                center_x,
                center_y,
                max_x,
                max_y,
                sin_y,
                cos_y,
                x,
                y,
                z_q7,
                random_states,
                &visible_count
            )
        );
    }
    for (size_t index = vector_end; index < count; index++) {
        float relative_x = x[index] - projection_center_x;
        float source_z = (float)z_q7[index] * DEPTH_FIXED_SCALE_RECIP;
        float rotated_x =
            relative_x * rotation_y_cos + source_z * rotation_y_sin;
        float rotated_z =
            -relative_x * rotation_y_sin + source_z * rotation_y_cos;
        float denominator = focal_length + rotated_z;
        if (denominator <= near_denominator) {
            commands[index] = PARTICLE_NOT_VISIBLE;
            continue;
        }
        float32x2_t denominator_vector = vdup_n_f32(denominator);
        float32x2_t reciprocal = vrecpe_f32(denominator_vector);
        reciprocal = vmul_f32(
            reciprocal,
            vrecps_f32(denominator_vector, reciprocal)
        );
        float scale = focal_length * vget_lane_f32(reciprocal, 0);
        float screen_x = projection_center_x + rotated_x * scale;
        float screen_y = projection_center_y +
            (y[index] - projection_center_y) * scale;
        if (screen_x <= -0.5f || screen_y <= -0.5f ||
            screen_x >= projection_max_x || screen_y >= projection_max_y) {
            commands[index] = PARTICLE_NOT_VISIBLE;
            continue;
        }
        uint32_t pixel_x = (uint32_t)(screen_x + 0.5f);
        uint32_t pixel_y = (uint32_t)(screen_y + 0.5f);
        uint32_t command = pixel_y * width_u32 + pixel_x;
        if (visual != 0) {
            uint32_t palette = random_states[index] >> 30;
            uint32_t neighbor = pixel_x + 1 < width_u32 &&
                (formed != 0
                    ? rotated_z < formed_neighbor_when_depth_below
                    : palette == unformed_palette_index);
            command |= palette << COMMAND_PALETTE_SHIFT;
            command |= neighbor != 0 ? COMMAND_NEIGHBOR : 0;
        }
        commands[index] = command;
        visible_count = vaddq_u32(
            visible_count,
            (uint32x4_t){1, 0, 0, 0}
        );
    }
    uint32_t lanes[4];
    vst1q_u32(lanes, visible_count);
    return (size_t)lanes[0] + lanes[1] + lanes[2] + lanes[3];
}
