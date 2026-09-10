/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2026 Nigel Breslaw */
#ifndef MISTER_MAGIK_MAIN_WINDOW_LAYOUT_H
#define MISTER_MAGIK_MAIN_WINDOW_LAYOUT_H

#include "mister_magik_main_window_policy.h"
#include "mister_magik_main_window_uapi.h"

/* Shared provider/client definition. Attribute flags describe the requested
 * mapping mechanism, not an attestation of hardware ownership or PTEs.
 */
#define MISTER_MAGIK_MAIN_WINDOW_LAYOUT_INITIALIZER { \
	.abi_version = MISTER_MAGIK_MAIN_WINDOW_ABI_VERSION, \
	.struct_bytes = 64U, \
	.physical_base = MISTER_MAGIK_MAIN_WINDOW_BASE, \
	.map_bytes = MISTER_MAGIK_MAIN_WINDOW_BYTES, \
	.flags = MISTER_MAGIK_MAIN_WINDOW_LAYOUT_WRITE_COMBINE, \
	.page_bytes = MISTER_MAGIK_MAIN_WINDOW_PAGE_BYTES, \
	.reserved = { 0 } \
}

static inline bool mister_magik_main_window_layout_valid(
	const struct mister_magik_main_window_layout *layout)
{
	unsigned int i;
	if (!layout || layout->abi_version != MISTER_MAGIK_MAIN_WINDOW_ABI_VERSION ||
	    layout->struct_bytes != 64U ||
	    layout->physical_base != MISTER_MAGIK_MAIN_WINDOW_BASE ||
	    layout->map_bytes != MISTER_MAGIK_MAIN_WINDOW_BYTES ||
	    layout->flags != MISTER_MAGIK_MAIN_WINDOW_LAYOUT_WRITE_COMBINE ||
	    layout->page_bytes != MISTER_MAGIK_MAIN_WINDOW_PAGE_BYTES)
		return false;
	for (i = 0; i < 10; i++)
		if (layout->reserved[i])
			return false;
	return true;
}

#endif
