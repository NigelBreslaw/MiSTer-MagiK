/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2026 Nigel Breslaw */

#ifndef MISTER_MAGIK_SCANOUT_SLOTS_UAPI_H
#define MISTER_MAGIK_SCANOUT_SLOTS_UAPI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define MISTER_MAGIK_SCANOUT_SLOTS_ABI_VERSION 2U
#define MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT 2U
#define MISTER_MAGIK_SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE 0x00000001U

struct mister_magik_scanout_slot {
	__u32 physical_address;
	__u32 mmap_offset_bytes;
};

struct mister_magik_scanout_slots_layout {
	__u32 abi_version;
	__u32 slot_count;
	__u32 max_width;
	__u32 max_height;
	__u32 max_stride_bytes;
	__u32 slot_capacity_bytes;
	__u32 map_bytes;
	__u32 flags;
	struct mister_magik_scanout_slot
		slots[MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT];
	__u32 reserved[4];
};

#define MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT \
	_IOR('M', 0x01, struct mister_magik_scanout_slots_layout)

#endif
