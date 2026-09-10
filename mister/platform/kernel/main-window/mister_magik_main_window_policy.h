/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2026 Nigel Breslaw */
#ifndef MISTER_MAGIK_MAIN_WINDOW_POLICY_H
#define MISTER_MAGIK_MAIN_WINDOW_POLICY_H

#ifdef __KERNEL__
#include <linux/types.h>
#else
#include <stdbool.h>
#endif

#define MISTER_MAGIK_MAIN_WINDOW_BASE 0x22000000UL
#define MISTER_MAGIK_MAIN_WINDOW_BYTES 0x017bb000UL
#define MISTER_MAGIK_MAIN_WINDOW_PAGE_BYTES 4096UL

/* Offsets are page offsets, as supplied by vm_pgoff, not byte addresses.
 * This predicate grants geometry/permission eligibility, not ownership.
 */
static inline bool mister_magik_main_window_mapping_valid(
	unsigned long page_offset, unsigned long bytes, unsigned long page_bytes,
	bool shared, bool readable, bool writable, bool executable)
{
	return page_offset == 0 && bytes == MISTER_MAGIK_MAIN_WINDOW_BYTES &&
		page_bytes == MISTER_MAGIK_MAIN_WINDOW_PAGE_BYTES &&
		shared && readable && writable && !executable;
}

#endif
