// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Nigel Breslaw
#include <assert.h>
#include <limits.h>
#include <stddef.h>
#include "mister_magik_main_window_policy.h"

#ifdef __linux__
#include <string.h>
#include "mister_magik_main_window_layout.h"
#include "../scanout-slots/mister_magik_scanout_slots_uapi.h"
_Static_assert(sizeof(struct mister_magik_main_window_layout) == 64, "Main ABI size");
_Static_assert(offsetof(struct mister_magik_main_window_layout, reserved) == 24, "Main ABI offsets");
_Static_assert(sizeof(struct mister_magik_scanout_slots_layout) == 64, "slot ABI size");
_Static_assert(MISTER_MAGIK_SCANOUT_SLOTS_ABI_VERSION == 3, "slot version preserved");
_Static_assert(MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT != MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT, "distinct ioctl");
_Static_assert(_IOC_SIZE(MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT) == 64, "ioctl size");
_Static_assert(_IOC_DIR(MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT) == _IOC_READ, "read-only query");
_Static_assert(_IOC_TYPE(MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT) == 'M', "ioctl type");
_Static_assert(_IOC_NR(MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT) == 2, "ioctl number");
#endif

int main(void)
{
#ifdef __linux__
	const struct mister_magik_main_window_layout expected = MISTER_MAGIK_MAIN_WINDOW_LAYOUT_INITIALIZER;
	struct mister_magik_main_window_layout changed;
	__u32 words[16];
	assert(mister_magik_main_window_layout_valid(&expected));
	assert(!mister_magik_main_window_layout_valid(NULL));
	for (unsigned int i = 0; i < 16; i++) {
		memcpy(words, &expected, sizeof(words));
		words[i] ^= 1;
		memcpy(&changed, words, sizeof(changed));
		assert(!mister_magik_main_window_layout_valid(&changed));
	}
#endif
	unsigned int flags;
	const unsigned long lengths[] = {0, 4096, MISTER_MAGIK_MAIN_WINDOW_BYTES - 1,
		MISTER_MAGIK_MAIN_WINDOW_BYTES + 1, ULONG_MAX};
	const unsigned long offsets[] = {1, 2025, 0x22000UL, ULONG_MAX};
	_Static_assert(MISTER_MAGIK_MAIN_WINDOW_BASE + MISTER_MAGIK_MAIN_WINDOW_BYTES == 0x237bb000UL, "window end");
	_Static_assert(MISTER_MAGIK_MAIN_WINDOW_BYTES % 4096 == 0, "page-aligned window");
	_Static_assert(0x227e9000UL >= MISTER_MAGIK_MAIN_WINDOW_BASE, "slot0 contained");
	_Static_assert(0x231d3000UL <= MISTER_MAGIK_MAIN_WINDOW_BASE + MISTER_MAGIK_MAIN_WINDOW_BYTES, "slot1 contained");
	for (flags = 0; flags < 16; flags++)
		assert(mister_magik_main_window_mapping_valid(0, MISTER_MAGIK_MAIN_WINDOW_BYTES,
			4096, flags & 1, flags & 2, flags & 4, flags & 8) == (flags == 7));
	for (size_t i = 0; i < sizeof(lengths) / sizeof(lengths[0]); i++)
		assert(!mister_magik_main_window_mapping_valid(0, lengths[i], 4096, true, true, true, false));
	for (size_t i = 0; i < sizeof(offsets) / sizeof(offsets[0]); i++)
		assert(!mister_magik_main_window_mapping_valid(offsets[i], MISTER_MAGIK_MAIN_WINDOW_BYTES, 4096, true, true, true, false));
	assert(!mister_magik_main_window_mapping_valid(0, MISTER_MAGIK_MAIN_WINDOW_BYTES, 8192, true, true, true, false));
	return 0;
}
