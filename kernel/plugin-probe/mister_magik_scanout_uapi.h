/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
#ifndef _UAPI_MISTER_MAGIK_SCANOUT_H
#define _UAPI_MISTER_MAGIK_SCANOUT_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define MISTER_MAGIK_SCANOUT_ABI_VERSION 1
#define MISTER_MAGIK_SCANOUT_SLOT_COUNT 2
#define MISTER_MAGIK_SCANOUT_MAX_RANGES 64

struct mister_magik_scanout_caps {
	__u32 abi_version;
	__u32 slot_count;
	__u32 slot_bytes;
	__u32 mmap_stride;
	__u32 dma_addr[MISTER_MAGIK_SCANOUT_SLOT_COUNT];
};

struct mister_magik_scanout_range {
	__u32 offset;
	__u32 length;
};

struct mister_magik_scanout_sync {
	__u32 slot;
	__u32 range_count;
	struct mister_magik_scanout_range ranges[MISTER_MAGIK_SCANOUT_MAX_RANGES];
};

#define MISTER_MAGIK_SCANOUT_IOC_MAGIC 'M'
#define MISTER_MAGIK_SCANOUT_GET_CAPS \
	_IOR(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x20, struct mister_magik_scanout_caps)
#define MISTER_MAGIK_SCANOUT_ACQUIRE_CPU \
	_IOW(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x21, __u32)
#define MISTER_MAGIK_SCANOUT_SYNC_DEVICE \
	_IOW(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x22, struct mister_magik_scanout_sync)

#endif
