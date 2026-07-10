#define _GNU_SOURCE

#include <arm_neon.h>
#include <errno.h>
#include <sched.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#if __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "RGB565 decimator benchmark requires little-endian pixels"
#endif

typedef void (*kernel_fn)(const uint16_t *, size_t, size_t, size_t, uint16_t *, size_t);

struct kernel {
    const char *name;
    kernel_fn run;
};

struct bench_case {
    const char *name;
    size_t width;
    size_t height;
    size_t stride;
};

void mister_magik_downsample_rgb565_2x_scalar(const uint16_t *source,
                                               size_t source_height,
                                               size_t source_stride,
                                               uint16_t *destination,
                                               size_t destination_width);
void mister_magik_downsample_rgb565_2x_neon(const uint16_t *source,
                                             size_t source_width,
                                             size_t source_height,
                                             size_t source_stride,
                                             uint16_t *destination,
                                             size_t destination_width);

static volatile uint64_t benchmark_sink;

static void production_scalar(const uint16_t *source,
                              size_t source_width,
                              size_t source_height,
                              size_t source_stride,
                              uint16_t *destination,
                              size_t destination_width) {
    (void)source_width;
    mister_magik_downsample_rgb565_2x_scalar(
        source, source_height, source_stride, destination, destination_width);
}

static void production_neon(const uint16_t *source,
                            size_t source_width,
                            size_t source_height,
                            size_t source_stride,
                            uint16_t *destination,
                            size_t destination_width) {
    mister_magik_downsample_rgb565_2x_neon(source,
                                           source_width,
                                           source_height,
                                           source_stride,
                                           destination,
                                           destination_width);
}

__attribute__((noinline)) static void scalar_pointer(const uint16_t *source,
                                                      size_t source_width,
                                                      size_t source_height,
                                                      size_t source_stride,
                                                      uint16_t *destination,
                                                      size_t destination_width) {
    (void)source_width;
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        const uint16_t *const end = dst + destination_width;
        while (dst < end) {
            *dst++ = *src;
            src += 2;
        }
    }
}

__attribute__((noinline)) static void scalar_pointer_restrict(
    const uint16_t *restrict source,
    size_t source_width,
    size_t source_height,
    size_t source_stride,
    uint16_t *restrict destination,
    size_t destination_width) {
    (void)source_width;
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        const uint16_t *const end = dst + destination_width;
        while (dst < end) {
            *dst++ = *src;
            src += 2;
        }
    }
}

__attribute__((noinline)) static void scalar_pointer_unroll2(
    const uint16_t *restrict source,
    size_t source_width,
    size_t source_height,
    size_t source_stride,
    uint16_t *restrict destination,
    size_t destination_width) {
    (void)source_width;
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        for (; x + 2U <= destination_width; x += 2U, src += 4U, dst += 2U) {
            dst[0] = src[0];
            dst[1] = src[2];
        }
        if (x < destination_width) {
            *dst = *src;
        }
    }
}

__attribute__((noinline)) static void scalar_pointer_2row(
    const uint16_t *restrict source,
    size_t source_width,
    size_t source_height,
    size_t source_stride,
    uint16_t *restrict destination,
    size_t destination_width) {
    (void)source_width;
    const size_t destination_height = (source_height + 1U) / 2U;
    size_t y = 0;
    for (; y + 2U <= destination_height; y += 2U) {
        const uint16_t *src0 = source + y * 2U * source_stride;
        const uint16_t *src1 = src0 + 2U * source_stride;
        uint16_t *dst0 = destination + y * destination_width;
        uint16_t *dst1 = dst0 + destination_width;
        for (size_t x = 0; x < destination_width; ++x, src0 += 2U, src1 += 2U) {
            dst0[x] = *src0;
            dst1[x] = *src1;
        }
    }
    if (y < destination_height) {
        scalar_pointer(source + y * 2U * source_stride,
                       0U,
                       1U,
                       source_stride,
                       destination + y * destination_width,
                       destination_width);
    }
}

__attribute__((noinline)) static void scalar_unroll8(const uint16_t *source,
                                                      size_t source_width,
                                                      size_t source_height,
                                                      size_t source_stride,
                                                      uint16_t *destination,
                                                      size_t destination_width) {
    (void)source_width;
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        for (; x + 8U <= destination_width; x += 8U, src += 16U, dst += 8U) {
            dst[0] = src[0];
            dst[1] = src[2];
            dst[2] = src[4];
            dst[3] = src[6];
            dst[4] = src[8];
            dst[5] = src[10];
            dst[6] = src[12];
            dst[7] = src[14];
        }
        for (; x < destination_width; ++x, src += 2U, ++dst) {
            *dst = *src;
        }
    }
}

__attribute__((noinline)) static void scalar_u32(const uint16_t *source,
                                                  size_t source_width,
                                                  size_t source_height,
                                                  size_t source_stride,
                                                  uint16_t *destination,
                                                  size_t destination_width) {
    (void)source_width;
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint32_t *src = (const uint32_t *)(source + y * 2U * source_stride);
        uint16_t *dst = destination + y * destination_width;
        for (size_t x = 0; x < destination_width; ++x) {
            dst[x] = (uint16_t)src[x];
        }
    }
}

static inline void scalar_tail(const uint16_t *source_row,
                               size_t output_x,
                               size_t destination_width,
                               uint16_t *destination_row) {
    while (output_x < destination_width) {
        destination_row[output_x] = source_row[output_x * 2U];
        ++output_x;
    }
}

__attribute__((noinline)) static void neon_narrow16(const uint16_t *source,
                                                     size_t source_width,
                                                     size_t source_height,
                                                     size_t source_stride,
                                                     uint16_t *destination,
                                                     size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        while (x + 16U <= destination_width && x * 2U + 32U <= source_width) {
            const uint32_t *pairs = (const uint32_t *)(src + x * 2U);
            const uint32x4_t a = vld1q_u32(pairs);
            const uint32x4_t b = vld1q_u32(pairs + 4U);
            const uint32x4_t c = vld1q_u32(pairs + 8U);
            const uint32x4_t d = vld1q_u32(pairs + 12U);
            vst1q_u16(dst + x, vcombine_u16(vmovn_u32(a), vmovn_u32(b)));
            vst1q_u16(dst + x + 8U, vcombine_u16(vmovn_u32(c), vmovn_u32(d)));
            x += 16U;
        }
        scalar_tail(src, x, destination_width, dst);
    }
}

__attribute__((noinline)) static void neon_narrow16_restrict(
    const uint16_t *restrict source,
    size_t source_width,
    size_t source_height,
    size_t source_stride,
    uint16_t *restrict destination,
    size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        while (x + 16U <= destination_width && x * 2U + 32U <= source_width) {
            const uint32_t *pairs = (const uint32_t *)(src + x * 2U);
            const uint32x4_t a = vld1q_u32(pairs);
            const uint32x4_t b = vld1q_u32(pairs + 4U);
            const uint32x4_t c = vld1q_u32(pairs + 8U);
            const uint32x4_t d = vld1q_u32(pairs + 12U);
            vst1q_u16(dst + x, vcombine_u16(vmovn_u32(a), vmovn_u32(b)));
            vst1q_u16(dst + x + 8U, vcombine_u16(vmovn_u32(c), vmovn_u32(d)));
            x += 16U;
        }
        scalar_tail(src, x, destination_width, dst);
    }
}

__attribute__((noinline)) static void neon_narrow16_2row(
    const uint16_t *restrict source,
    size_t source_width,
    size_t source_height,
    size_t source_stride,
    uint16_t *restrict destination,
    size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;
    size_t y = 0;
    for (; y + 2U <= destination_height; y += 2U) {
        const uint16_t *src0 = source + y * 2U * source_stride;
        const uint16_t *src1 = src0 + 2U * source_stride;
        uint16_t *dst0 = destination + y * destination_width;
        uint16_t *dst1 = dst0 + destination_width;
        size_t x = 0;
        while (x + 16U <= destination_width && x * 2U + 32U <= source_width) {
            const uint32_t *pairs0 = (const uint32_t *)(src0 + x * 2U);
            const uint32_t *pairs1 = (const uint32_t *)(src1 + x * 2U);
            const uint32x4_t a0 = vld1q_u32(pairs0);
            const uint32x4_t a1 = vld1q_u32(pairs1);
            const uint32x4_t b0 = vld1q_u32(pairs0 + 4U);
            const uint32x4_t b1 = vld1q_u32(pairs1 + 4U);
            vst1q_u16(dst0 + x, vcombine_u16(vmovn_u32(a0), vmovn_u32(b0)));
            vst1q_u16(dst1 + x, vcombine_u16(vmovn_u32(a1), vmovn_u32(b1)));
            const uint32x4_t c0 = vld1q_u32(pairs0 + 8U);
            const uint32x4_t c1 = vld1q_u32(pairs1 + 8U);
            const uint32x4_t d0 = vld1q_u32(pairs0 + 12U);
            const uint32x4_t d1 = vld1q_u32(pairs1 + 12U);
            vst1q_u16(dst0 + x + 8U, vcombine_u16(vmovn_u32(c0), vmovn_u32(d0)));
            vst1q_u16(dst1 + x + 8U, vcombine_u16(vmovn_u32(c1), vmovn_u32(d1)));
            x += 16U;
        }
        scalar_tail(src0, x, destination_width, dst0);
        scalar_tail(src1, x, destination_width, dst1);
    }
    if (y < destination_height) {
        neon_narrow16_restrict(source + y * 2U * source_stride,
                               source_width,
                               1U,
                               source_stride,
                               destination + y * destination_width,
                               destination_width);
    }
}

__attribute__((always_inline)) static inline void neon_narrow32_prefetch_impl(
    const uint16_t *source,
    size_t source_width,
    size_t source_height,
    size_t source_stride,
    uint16_t *destination,
    size_t destination_width,
    size_t lookahead) {
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        while (x + 32U <= destination_width && x * 2U + 64U <= source_width) {
            const uint32_t *pairs = (const uint32_t *)(src + x * 2U);
            if (x + (lookahead) < destination_width) {
                __builtin_prefetch(src + (x + (lookahead)) * 2U, 0, 0);
            }
            const uint32x4_t a = vld1q_u32(pairs);
            const uint32x4_t b = vld1q_u32(pairs + 4U);
            const uint32x4_t c = vld1q_u32(pairs + 8U);
            const uint32x4_t d = vld1q_u32(pairs + 12U);
            const uint32x4_t e = vld1q_u32(pairs + 16U);
            const uint32x4_t f = vld1q_u32(pairs + 20U);
            const uint32x4_t g = vld1q_u32(pairs + 24U);
            const uint32x4_t h = vld1q_u32(pairs + 28U);
            vst1q_u16(dst + x, vcombine_u16(vmovn_u32(a), vmovn_u32(b)));
            vst1q_u16(dst + x + 8U, vcombine_u16(vmovn_u32(c), vmovn_u32(d)));
            vst1q_u16(dst + x + 16U, vcombine_u16(vmovn_u32(e), vmovn_u32(f)));
            vst1q_u16(dst + x + 24U, vcombine_u16(vmovn_u32(g), vmovn_u32(h)));
            x += 32U;
        }
        scalar_tail(src, x, destination_width, dst);
    }
}

#define DEFINE_NEON_NARROW32_PREFETCH(name, lookahead)                                   \
    __attribute__((noinline)) static void name(const uint16_t *source,                   \
                                                size_t source_width,                      \
                                                size_t source_height,                     \
                                                size_t source_stride,                     \
                                                uint16_t *destination,                    \
                                                size_t destination_width) {               \
        neon_narrow32_prefetch_impl(source,                                               \
                                    source_width,                                         \
                                    source_height,                                        \
                                    source_stride,                                        \
                                    destination,                                          \
                                    destination_width,                                    \
                                    (lookahead));                                         \
    }

DEFINE_NEON_NARROW32_PREFETCH(neon_narrow32_prefetch32, 32U)
DEFINE_NEON_NARROW32_PREFETCH(neon_narrow32_prefetch64, 64U)
DEFINE_NEON_NARROW32_PREFETCH(neon_narrow32_prefetch96, 96U)
DEFINE_NEON_NARROW32_PREFETCH(neon_narrow32_prefetch128, 128U)

#undef DEFINE_NEON_NARROW32_PREFETCH

__attribute__((noinline)) static void neon_narrow32(const uint16_t *source,
                                                     size_t source_width,
                                                     size_t source_height,
                                                     size_t source_stride,
                                                     uint16_t *destination,
                                                     size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        while (x + 32U <= destination_width && x * 2U + 64U <= source_width) {
            const uint32_t *pairs = (const uint32_t *)(src + x * 2U);
            const uint32x4_t a = vld1q_u32(pairs);
            const uint32x4_t b = vld1q_u32(pairs + 4U);
            const uint32x4_t c = vld1q_u32(pairs + 8U);
            const uint32x4_t d = vld1q_u32(pairs + 12U);
            const uint32x4_t e = vld1q_u32(pairs + 16U);
            const uint32x4_t f = vld1q_u32(pairs + 20U);
            const uint32x4_t g = vld1q_u32(pairs + 24U);
            const uint32x4_t h = vld1q_u32(pairs + 28U);
            vst1q_u16(dst + x, vcombine_u16(vmovn_u32(a), vmovn_u32(b)));
            vst1q_u16(dst + x + 8U, vcombine_u16(vmovn_u32(c), vmovn_u32(d)));
            vst1q_u16(dst + x + 16U, vcombine_u16(vmovn_u32(e), vmovn_u32(f)));
            vst1q_u16(dst + x + 24U, vcombine_u16(vmovn_u32(g), vmovn_u32(h)));
            x += 32U;
        }
        scalar_tail(src, x, destination_width, dst);
    }
}

__attribute__((noinline)) static void neon_vld2(const uint16_t *source,
                                                 size_t source_width,
                                                 size_t source_height,
                                                 size_t source_stride,
                                                 uint16_t *destination,
                                                 size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        while (x + 8U <= destination_width && x * 2U + 16U <= source_width) {
            const uint16x8x2_t separated = vld2q_u16(src + x * 2U);
            vst1q_u16(dst + x, separated.val[0]);
            x += 8U;
        }
        scalar_tail(src, x, destination_width, dst);
    }
}

__attribute__((noinline)) static void neon_vuzp(const uint16_t *source,
                                                 size_t source_width,
                                                 size_t source_height,
                                                 size_t source_stride,
                                                 uint16_t *destination,
                                                 size_t destination_width) {
    const size_t destination_height = (source_height + 1U) / 2U;
    for (size_t y = 0; y < destination_height; ++y) {
        const uint16_t *src = source + y * 2U * source_stride;
        uint16_t *dst = destination + y * destination_width;
        size_t x = 0;
        while (x + 8U <= destination_width && x * 2U + 16U <= source_width) {
            const uint16x8_t first = vld1q_u16(src + x * 2U);
            const uint16x8_t second = vld1q_u16(src + x * 2U + 8U);
            const uint16x8x2_t separated = vuzpq_u16(first, second);
            vst1q_u16(dst + x, separated.val[0]);
            x += 8U;
        }
        scalar_tail(src, x, destination_width, dst);
    }
}

static uint64_t now_ns(void) {
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC_RAW, &value) != 0) {
        perror("clock_gettime");
        exit(2);
    }
    return (uint64_t)value.tv_sec * 1000000000ULL + (uint64_t)value.tv_nsec;
}

static int compare_u64(const void *left, const void *right) {
    const uint64_t a = *(const uint64_t *)left;
    const uint64_t b = *(const uint64_t *)right;
    return (a > b) - (a < b);
}

static uint64_t percentile(const uint64_t *sorted, size_t count, size_t percent) {
    size_t rank = (percent * count + 99U) / 100U;
    if (rank == 0U) {
        rank = 1U;
    }
    if (rank > count) {
        rank = count;
    }
    return sorted[rank - 1U];
}

static uint64_t checksum(const uint16_t *pixels, size_t count) {
    uint64_t hash = 0xcbf29ce484222325ULL;
    for (size_t i = 0; i < count; ++i) {
        hash ^= (uint8_t)(pixels[i] & 0xffU);
        hash *= 0x100000001b3ULL;
        hash ^= (uint8_t)(pixels[i] >> 8U);
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

static void fill_source(uint16_t *pixels, size_t count) {
    uint32_t state = 0x12345678U;
    for (size_t i = 0; i < count; ++i) {
        state = state * 1664525U + 1013904223U;
        pixels[i] = (uint16_t)(state >> 16U);
    }
}

static void *aligned_alloc_or_die(size_t alignment, size_t size) {
    void *pointer = NULL;
    const int result = posix_memalign(&pointer, alignment, size);
    if (result != 0) {
        errno = result;
        perror("posix_memalign");
        exit(2);
    }
    return pointer;
}

static void pin_cpu(int cpu) {
    if (cpu < 0) {
        return;
    }
    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET((unsigned int)cpu, &set);
    if (sched_setaffinity(0, sizeof(set), &set) != 0) {
        perror("sched_setaffinity");
        exit(2);
    }
}

static void run_case(const struct bench_case *bench_case,
                     const struct kernel *kernels,
                     size_t kernel_count,
                     size_t samples,
                     size_t repeat_id,
                     int cpu) {
    const size_t source_count = bench_case->stride * bench_case->height;
    const size_t destination_width = (bench_case->width + 1U) / 2U;
    const size_t destination_height = (bench_case->height + 1U) / 2U;
    const size_t destination_count = destination_width * destination_height;
    uint16_t *source = aligned_alloc_or_die(64U, source_count * sizeof(*source));
    uint16_t *reference = aligned_alloc_or_die(64U, destination_count * sizeof(*reference));
    uint16_t **destinations = calloc(kernel_count, sizeof(*destinations));
    uint64_t **durations = calloc(kernel_count, sizeof(*durations));
    if (destinations == NULL || durations == NULL) {
        perror("calloc");
        exit(2);
    }

    fill_source(source, source_count);
    production_scalar(source,
                      bench_case->width,
                      bench_case->height,
                      bench_case->stride,
                      reference,
                      destination_width);
    const uint64_t reference_checksum = checksum(reference, destination_count);

    for (size_t index = 0; index < kernel_count; ++index) {
        destinations[index] = aligned_alloc_or_die(64U, destination_count * sizeof(uint16_t));
        durations[index] = calloc(samples, sizeof(uint64_t));
        if (durations[index] == NULL) {
            perror("calloc");
            exit(2);
        }
        kernels[index].run(source,
                           bench_case->width,
                           bench_case->height,
                           bench_case->stride,
                           destinations[index],
                           destination_width);
        const uint64_t actual_checksum = checksum(destinations[index], destination_count);
        if (actual_checksum != reference_checksum ||
            memcmp(destinations[index], reference, destination_count * sizeof(uint16_t)) != 0) {
            fprintf(stderr,
                    "checksum mismatch case=%s kernel=%s expected=%016llx actual=%016llx\n",
                    bench_case->name,
                    kernels[index].name,
                    (unsigned long long)reference_checksum,
                    (unsigned long long)actual_checksum);
            exit(3);
        }
    }

    for (size_t warmup = 0; warmup < 25U; ++warmup) {
        for (size_t offset = 0; offset < kernel_count; ++offset) {
            const size_t index = (warmup + offset) % kernel_count;
            kernels[index].run(source,
                               bench_case->width,
                               bench_case->height,
                               bench_case->stride,
                               destinations[index],
                               destination_width);
        }
    }

    for (size_t sample = 0; sample < samples; ++sample) {
        for (size_t offset = 0; offset < kernel_count; ++offset) {
            const size_t index = (sample + repeat_id + offset) % kernel_count;
            const uint64_t started = now_ns();
            kernels[index].run(source,
                               bench_case->width,
                               bench_case->height,
                               bench_case->stride,
                               destinations[index],
                               destination_width);
            durations[index][sample] = now_ns() - started;
            benchmark_sink ^= destinations[index][sample % destination_count];
        }
    }

    for (size_t index = 0; index < kernel_count; ++index) {
        qsort(durations[index], samples, sizeof(uint64_t), compare_u64);
        printf("rgb565_decimator_bench_tsv\trepeat=%zu\tcpu=%d\tcase=%s\tkernel=%s\tsamples=%zu\tchecksum=%016llx\tp50_ns=%llu\tp95_ns=%llu\tmax_ns=%llu\n",
               repeat_id,
               cpu,
               bench_case->name,
               kernels[index].name,
               samples,
               (unsigned long long)reference_checksum,
               (unsigned long long)percentile(durations[index], samples, 50U),
               (unsigned long long)percentile(durations[index], samples, 95U),
               (unsigned long long)durations[index][samples - 1U]);
    }

    for (size_t index = 0; index < kernel_count; ++index) {
        free(durations[index]);
        free(destinations[index]);
    }
    free(durations);
    free(destinations);
    free(reference);
    free(source);
}

static size_t parse_size(const char *value, const char *name) {
    char *end = NULL;
    const unsigned long parsed = strtoul(value, &end, 10);
    if (value[0] == '\0' || end == NULL || *end != '\0' || parsed == 0UL) {
        fprintf(stderr, "invalid %s: %s\n", name, value);
        exit(2);
    }
    return (size_t)parsed;
}

static void verify_misaligned_production_neon(void) {
    const size_t width = 65U;
    const size_t height = 33U;
    const size_t stride = 69U;
    const size_t source_count = stride * height;
    const size_t destination_width = (width + 1U) / 2U;
    const size_t destination_count = destination_width * ((height + 1U) / 2U);
    uint16_t *allocation = aligned_alloc_or_die(64U, (source_count + 1U) * sizeof(uint16_t));
    uint16_t *source = allocation + 1U;
    uint16_t *scalar = aligned_alloc_or_die(64U, destination_count * sizeof(uint16_t));
    uint16_t *neon = aligned_alloc_or_die(64U, destination_count * sizeof(uint16_t));

    fill_source(source, source_count);
    production_scalar(source, width, height, stride, scalar, destination_width);
    production_neon(source, width, height, stride, neon, destination_width);
    if (memcmp(scalar, neon, destination_count * sizeof(uint16_t)) != 0) {
        fprintf(stderr, "misaligned production NEON fallback mismatch\n");
        exit(3);
    }
    printf("rgb565_decimator_alignment_tsv\toffset_bytes=2\tchecksum=%016llx\tmatched=1\n",
           (unsigned long long)checksum(neon, destination_count));

    free(neon);
    free(scalar);
    free(allocation);
}

int main(int argc, char **argv) {
    size_t samples = 200U;
    size_t repeat_id = 1U;
    int cpu = 0;
    for (int index = 1; index < argc; ++index) {
        if (strcmp(argv[index], "--samples") == 0 && index + 1 < argc) {
            samples = parse_size(argv[++index], "samples");
        } else if (strcmp(argv[index], "--repeat") == 0 && index + 1 < argc) {
            repeat_id = parse_size(argv[++index], "repeat");
        } else if (strcmp(argv[index], "--cpu") == 0 && index + 1 < argc) {
            cpu = (int)strtol(argv[++index], NULL, 10);
        } else {
            fprintf(stderr,
                    "usage: rgb565-decimator-bench [--samples N] [--repeat N] [--cpu N]\n");
            return 2;
        }
    }
    pin_cpu(cpu);
    verify_misaligned_production_neon();

    const struct kernel kernels[] = {
        {"scalar-production", production_scalar},
        {"scalar-pointer", scalar_pointer},
        {"scalar-pointer-restrict", scalar_pointer_restrict},
        {"scalar-pointer-unroll2", scalar_pointer_unroll2},
        {"scalar-pointer-2row", scalar_pointer_2row},
        {"scalar-unroll8", scalar_unroll8},
        {"scalar-u32", scalar_u32},
        {"neon-production", production_neon},
        {"neon-narrow16", neon_narrow16},
        {"neon-narrow16-restrict", neon_narrow16_restrict},
        {"neon-narrow16-2row", neon_narrow16_2row},
        {"neon-narrow32", neon_narrow32},
        {"neon-narrow32-prefetch32", neon_narrow32_prefetch32},
        {"neon-narrow32-prefetch64", neon_narrow32_prefetch64},
        {"neon-narrow32-prefetch96", neon_narrow32_prefetch96},
        {"neon-narrow32-prefetch128", neon_narrow32_prefetch128},
        {"neon-vld2", neon_vld2},
        {"neon-vuzp", neon_vuzp},
    };
    const struct bench_case cases[] = {
        {"full-960x540", 960U, 540U, 960U},
        {"padded-960x540", 960U, 540U, 976U},
        {"odd-959x539", 959U, 539U, 967U},
    };

    for (size_t index = 0; index < sizeof(cases) / sizeof(cases[0]); ++index) {
        run_case(&cases[index],
                 kernels,
                 sizeof(kernels) / sizeof(kernels[0]),
                 samples,
                 repeat_id,
                 cpu);
    }
    printf("rgb565_decimator_sink_tsv\tvalue=%llu\n", (unsigned long long)benchmark_sink);
    return 0;
}
