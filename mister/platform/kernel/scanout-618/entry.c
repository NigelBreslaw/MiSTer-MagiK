// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Nigel Breslaw
#include <linux/module.h>
#include "provider.h"

static int __init window_provider_init(void)
{
	/* No production platform validator exists yet. No parameter can bypass
	 * this gate; compiling the mapping mechanism is not qualification.
	 */
	return mister_magik_window_provider_register(false);
}

static void __exit window_provider_exit(void)
{
	mister_magik_window_provider_unregister();
}

module_init(window_provider_init);
module_exit(window_provider_exit);
MODULE_DESCRIPTION("MiSTer MagiK unqualified fixed-window provider (activation disabled)");
MODULE_LICENSE("Proprietary");
MODULE_INFO(mister_magik_source_license, "GPL-3.0-or-later");
