/* SPDX-License-Identifier: GPL-2.0-only OR GPL-3.0-or-later */
/* Copyright (C) 2026 Nigel Breslaw */
#ifndef MISTER_MAGIK_MAPPING_DIAGNOSTIC_UAPI_H
#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_UAPI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_ABI_VERSION 2U
#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_NO_PAGE (~0U)

#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_NOT_ATTEMPTED 0U
#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_IN_PROGRESS 1U
#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_PASSED 2U
#define MISTER_MAGIK_MAPPING_DIAGNOSTIC_FAILED 3U

#define MISTER_MAGIK_MAPPING_FAILURE_REMAP (1U << 0)
#define MISTER_MAGIK_MAPPING_FAILURE_LOOKUP (1U << 1)
#define MISTER_MAGIK_MAPPING_FAILURE_PFN (1U << 2)
#define MISTER_MAGIK_MAPPING_FAILURE_WRITABLE (1U << 3)
#define MISTER_MAGIK_MAPPING_FAILURE_WC (1U << 4)
#define MISTER_MAGIK_MAPPING_FAILURE_XN (1U << 5)
#define MISTER_MAGIK_MAPPING_FAILURE_READONLY (1U << 6)
#define MISTER_MAGIK_MAPPING_FAILURE_SHARED (1U << 7)

#define MISTER_MAGIK_MAPPING_VERIFIED_VMA_FLAGS (1U << 0)
#define MISTER_MAGIK_MAPPING_VERIFIED_WC (1U << 1)
#define MISTER_MAGIK_MAPPING_VERIFIED_XN (1U << 2)
#define MISTER_MAGIK_MAPPING_VERIFIED_WRITABLE_PROTECTION (1U << 3)
#define MISTER_MAGIK_MAPPING_VERIFIED_SHARED (1U << 4)
#define MISTER_MAGIK_MAPPING_VERIFIED_PROTECTION_SUPPLIED (1U << 5)
#define MISTER_MAGIK_MAPPING_VERIFIED_PFNS (1U << 6)
#define MISTER_MAGIK_MAPPING_VERIFIED_WRITABILITY (1U << 7)
#define MISTER_MAGIK_MAPPING_PROTECTION_READBACK_UNAVAILABLE (1U << 8)

struct mister_magik_mapping_diagnostic {
	__u32 abi_version;
	__u32 record_bytes;
	__u32 state;
	__u32 failure_flags;
	__u32 page_index;
	__u32 expected_pfn;
	__u32 observed_pfn;
	__u32 writable;
	__u32 protection_mask;
	__u32 expected_protection;
	__u32 observed_protection;
	__s32 error_code;
	__u32 physical_base;
	__u32 map_bytes;
	__u32 verification_flags;
	__u32 reserved;
};

#define MISTER_MAGIK_MAPPING_GET_DIAGNOSTIC \
	_IOR('M', 0x03, struct mister_magik_mapping_diagnostic)

#endif
