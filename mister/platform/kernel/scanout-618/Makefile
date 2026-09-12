# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Nigel Breslaw
ifdef MISTER_MAGIK_DEVELOPMENT_TRIAL
ccflags-y += -DMISTER_MAGIK_DEVELOPMENT_TRIAL
obj-m += mister_magik_scanout_slots.o
mister_magik_scanout_slots-y := entry.o provider.o
else
obj-m += mister_magik_window_provider.o
mister_magik_window_provider-y := entry.o provider.o
endif
