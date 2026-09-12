/* SPDX-License-Identifier: GPL-2.0-only */
/* Copyright (C) 2026 Nigel Breslaw */
#ifndef MISTER_MAGIK_WINDOW_PROVIDER_H
#define MISTER_MAGIK_WINDOW_PROVIDER_H
#include <linux/types.h>
/* Internal module entry points, never exported or user-selectable. */
int mister_magik_window_provider_register(bool platform_qualified);
void mister_magik_window_provider_unregister(void);
#endif
