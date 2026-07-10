#define _GNU_SOURCE

#include <errno.h>
#include <sched.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

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

static volatile uint64_t benchmark_sink;

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
    for (size_t index = 0; index < count; ++index) {
        hash ^= (uint8_t)(pixels[index] & 0xffU);
        hash *= 0x100000001b3ULL;
        hash ^= (uint8_t)(pixels[index] >> 8U);
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

static void fill_source(uint16_t *pixels, size_t count) {
    uint32_t state = 0x12345678U;
    for (size_t index = 0; index < count; ++index) {
        state = state * 1664525U + 1013904223U;
        pixels[index] = (uint16_t)(state >> 16U);
    }
}

static void reference_scalar(const uint16_t *source,
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
                     size_t samples,
                     size_t repeat_id,
                     int cpu) {
    const size_t source_count = bench_case->stride * bench_case->height;
    const size_t destination_width = (bench_case->width + 1U) / 2U;
    const size_t destination_height = (bench_case->height + 1U) / 2U;
    const size_t destination_count = destination_width * destination_height;
    uint16_t *source = aligned_alloc_or_die(64U, source_count * sizeof(uint16_t));
    uint16_t *reference = aligned_alloc_or_die(64U, destination_count * sizeof(uint16_t));
    uint16_t *destination = aligned_alloc_or_die(64U, destination_count * sizeof(uint16_t));
    uint64_t *durations = calloc(samples, sizeof(uint64_t));
    if (durations == NULL) {
        perror("calloc");
        exit(2);
    }

    fill_source(source, source_count);
    reference_scalar(source,
                     bench_case->height,
                     bench_case->stride,
                     reference,
                     destination_width);
    mister_magik_downsample_rgb565_2x_scalar(source,
                                              bench_case->height,
                                              bench_case->stride,
                                              destination,
                                              destination_width);
    if (memcmp(destination, reference, destination_count * sizeof(uint16_t)) != 0) {
        fprintf(stderr, "scalar mismatch case=%s\n", bench_case->name);
        exit(3);
    }

    for (size_t warmup = 0; warmup < 25U; ++warmup) {
        mister_magik_downsample_rgb565_2x_scalar(source,
                                                  bench_case->height,
                                                  bench_case->stride,
                                                  destination,
                                                  destination_width);
    }
    for (size_t sample = 0; sample < samples; ++sample) {
        const uint64_t started = now_ns();
        mister_magik_downsample_rgb565_2x_scalar(source,
                                                  bench_case->height,
                                                  bench_case->stride,
                                                  destination,
                                                  destination_width);
        durations[sample] = now_ns() - started;
        benchmark_sink ^= destination[sample % destination_count];
    }

    qsort(durations, samples, sizeof(uint64_t), compare_u64);
    printf("rgb565_decimator_bench_tsv\trepeat=%zu\tcpu=%d\tcase=%s\tkernel=scalar-production\tsamples=%zu\tchecksum=%016llx\tp50_ns=%llu\tp95_ns=%llu\tmax_ns=%llu\n",
           repeat_id,
           cpu,
           bench_case->name,
           samples,
           (unsigned long long)checksum(destination, destination_count),
           (unsigned long long)percentile(durations, samples, 50U),
           (unsigned long long)percentile(durations, samples, 95U),
           (unsigned long long)durations[samples - 1U]);

    free(durations);
    free(destination);
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

    const struct bench_case cases[] = {
        {"full-960x540", 960U, 540U, 960U},
        {"padded-960x540", 960U, 540U, 976U},
        {"odd-959x539", 959U, 539U, 967U},
    };
    for (size_t index = 0; index < sizeof(cases) / sizeof(cases[0]); ++index) {
        run_case(&cases[index], samples, repeat_id, cpu);
    }
    printf("rgb565_decimator_sink_tsv\tvalue=%llu\n", (unsigned long long)benchmark_sink);
    return 0;
}
