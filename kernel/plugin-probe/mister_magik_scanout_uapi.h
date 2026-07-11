/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
#ifndef _UAPI_MISTER_MAGIK_SCANOUT_H
#define _UAPI_MISTER_MAGIK_SCANOUT_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define MISTER_MAGIK_SCANOUT_ABI_VERSION 1
#define MISTER_MAGIK_SCANOUT_ABI_VERSION_V2 2
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

#define MISTER_MAGIK_SCANOUT_CAP_RANGE_SYNC      (1U << 0)
#define MISTER_MAGIK_SCANOUT_CAP_HARD_OWNERSHIP  (1U << 1)
#define MISTER_MAGIK_SCANOUT_CAP_ATOMIC_POST     (1U << 2)
#define MISTER_MAGIK_SCANOUT_CAP_ACP_MAILBOX     (1U << 3)

#define MISTER_MAGIK_SCANOUT_SLOT_CPU_OWNED       0
#define MISTER_MAGIK_SCANOUT_SLOT_DEVICE_QUEUED   1
#define MISTER_MAGIK_SCANOUT_SLOT_DEVICE_ACTIVE   2
#define MISTER_MAGIK_SCANOUT_SLOT_CPU_RELEASED    3

struct mister_magik_scanout_caps_v2 {
	__u32 abi_version;
	__u32 capabilities;
	__u32 slot_count;
	__u32 slot_bytes;
	__u32 mmap_stride;
	__u32 mailbox_phys;
	__u32 mailbox_epoch;
	__u32 dma_addr[MISTER_MAGIK_SCANOUT_SLOT_COUNT];
};

struct mister_magik_scanout_mailbox_arm {
	__u32 epoch;
	__u32 fpga_capabilities;
};

struct mister_magik_scanout_post {
	__u32 slot;
	__u32 range_count;
	__u32 sequence;
	__u32 enable;
	__u32 filter;
	__u32 format;
	__u32 width;
	__u32 height;
	__u32 stride;
	__u32 hmin;
	__u32 hmax;
	__u32 vmin;
	__u32 vmax;
	struct mister_magik_scanout_range ranges[MISTER_MAGIK_SCANOUT_MAX_RANGES];
};

struct mister_magik_scanout_status {
	__u32 mailbox_armed;
	__u32 active_sequence;
	__u32 pending_sequence;
	__u32 active_slot;
	__u32 pending_slot;
	__u32 slot_state[MISTER_MAGIK_SCANOUT_SLOT_COUNT];
	__u32 completion_count;
	__u32 error_count;
};

#define MISTER_MAGIK_SCANOUT_IOC_MAGIC 'M'
#define MISTER_MAGIK_SCANOUT_GET_CAPS \
	_IOR(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x20, struct mister_magik_scanout_caps)
#define MISTER_MAGIK_SCANOUT_ACQUIRE_CPU \
	_IOW(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x21, __u32)
#define MISTER_MAGIK_SCANOUT_SYNC_DEVICE \
	_IOW(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x22, struct mister_magik_scanout_sync)
#define MISTER_MAGIK_SCANOUT_GET_CAPS_V2 \
	_IOR(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x23, struct mister_magik_scanout_caps_v2)
#define MISTER_MAGIK_SCANOUT_ARM_MAILBOX \
	_IOW(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x24, struct mister_magik_scanout_mailbox_arm)
#define MISTER_MAGIK_SCANOUT_SYNC_RANGES_AND_POST \
	_IOW(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x25, struct mister_magik_scanout_post)
#define MISTER_MAGIK_SCANOUT_GET_STATUS \
	_IOR(MISTER_MAGIK_SCANOUT_IOC_MAGIC, 0x26, struct mister_magik_scanout_status)

#endif
