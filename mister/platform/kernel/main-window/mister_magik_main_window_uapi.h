/* SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-or-later */
/* Copyright (C) 2026 Nigel Breslaw */
#ifndef MISTER_MAGIK_MAIN_WINDOW_UAPI_H
#define MISTER_MAGIK_MAIN_WINDOW_UAPI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define MISTER_MAGIK_MAIN_WINDOW_ABI_VERSION 1U
#define MISTER_MAGIK_MAIN_WINDOW_LAYOUT_WRITE_COMBINE 0x00000001U

/* Separate device and command: does not reinterpret the slot-v3 ABI.
 * Fixed-width fields, no pointers, and zero reserved words on output.
 */
struct mister_magik_main_window_layout {
	__u32 abi_version;
	__u32 struct_bytes;
	__u32 physical_base;
	__u32 map_bytes;
	__u32 flags;
	__u32 page_bytes;
	__u32 reserved[10];
};

#define MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT \
	_IOR('M', 0x02, struct mister_magik_main_window_layout)

#endif
