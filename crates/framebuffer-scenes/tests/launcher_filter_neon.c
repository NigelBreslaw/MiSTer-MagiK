// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
// Standalone ARM/AArch64 NEON parity test:
// cc -O3 tests/launcher_filter_neon.c -o /tmp/launcher-filter-test
// ARMv7 additionally requires -mfpu=neon-vfpv3 -mfloat-abi=hard.
#include "../src/launcher_texture_neon.c"
#include <assert.h>
#include <stdio.h>

static uint32_t reference_mix(uint32_t a, uint32_t b, uint32_t weight) {
  uint32_t result = 0;
  for (unsigned shift = 0; shift < 32; shift += 8)
    result |= ((((a >> shift) & 255) * (256 - weight) +
                ((b >> shift) & 255) * weight) / 256) << shift;
  return result;
}

int main(void) {
  uint32_t columns[5][35], out[37], state = 17;
  for (size_t column = 0; column < 5; ++column)
    for (size_t i = 0; i < 35; ++i) {
      state = state * 1664525u + 1013904223u;
      columns[column][i] = state;
    }
  const uint32_t weights[] = {0, 1, 127, 255, 256};
  size_t cases = 0;
  for (size_t n = 0; n <= 35; ++n)
    for (size_t wx = 0; wx < 5; ++wx)
      for (size_t lod = 0; lod < 5; ++lod)
        for (size_t face = 0; face <= 5; ++face)
          for (uint32_t light = 0; light <= 256; ++light) {
            const uint32_t x = weights[wx], x2 = weights[4 - wx];
            const uint32_t mip = weights[lod], weight = weights[face % 5];
            const uint32_t *other = face == 5 ? NULL : columns[4];
            for (size_t i = 0; i < 37; ++i)
              out[i] = 0xdeadbeef;
            magik_launcher_filter_lit_column(out + 1, columns[0], columns[1],
                columns[2], columns[3], n, x, x2, mip, light, other, weight);
            for (size_t i = 0; i < n; ++i) {
              uint32_t p = reference_mix(
                  reference_mix(columns[0][i], columns[1][i], x),
                  reference_mix(columns[2][i], columns[3][i], x2), mip);
              if (other)
                p = reference_mix(p, other[i], weight);
              uint32_t expected = p & 0xff000000;
              for (unsigned shift = 0; shift < 24; shift += 8)
                expected |= (((p >> shift) & 255) * light / 256) << shift;
              assert(out[i + 1] == expected);
            }
            assert(out[0] == 0xdeadbeef && out[n + 1] == 0xdeadbeef);
            ++cases;
          }
  printf("%zu NEON cases passed; exact RGBA, all light values and vector tails\n", cases);
  return 0;
}
