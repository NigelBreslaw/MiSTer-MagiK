// SPDX-License-Identifier: GPL-2.0-only
// Copyright (C) 2026 Nigel Breslaw
#include <linux/module.h>
#include "provider.h"

static int __init window_provider_init(void)
{
	/* The ordinary object remains impossible to activate. The separate trial
	 * object is produced only by the attended qualification build and still
	 * runs every exact platform check in provider.c before publishing nodes.
	 */
#ifdef MISTER_MAGIK_DEVELOPMENT_TRIAL
	return mister_magik_window_provider_register(true);
#else
	return mister_magik_window_provider_register(false);
#endif
}

static void __exit window_provider_exit(void)
{
	mister_magik_window_provider_unregister();
}

module_init(window_provider_init);
module_exit(window_provider_exit);
MODULE_DESCRIPTION("MiSTer MagiK unqualified fixed-window provider (activation disabled)");
MODULE_LICENSE("GPL");
MODULE_INFO(mister_magik_source_license, "GPL-2.0-only");
#ifdef MISTER_MAGIK_DEVELOPMENT_TRIAL
MODULE_INFO(mister_magik_development_trial, "stock-6.18-latch-reuse-v3");
#endif
