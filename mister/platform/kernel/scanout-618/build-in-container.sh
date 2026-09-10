#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Nigel Breslaw
set -euo pipefail
[[ $# == 1 ]] || exit 2
out="$1"
mkdir "$out"
cd /inputs
echo '1b85088193ab48dff58698c183b08eb6a8dc808c013fa57891d5f53ef7536fbd  kernel.tar' | sha256sum -c -
echo 'd169f9196e3a6c4248ee79ca85987ebce0e4ea9174c1f8d51af9b28fecf22da1  toolchain.tar.xz' | sha256sum -c -
echo '29389f52958f87f42f08ca29684f7704044cb1e2102a7c6135bda3253cc9c24a  kernel.config' | sha256sum -c -
echo 'fd2e67a4f798eb41c5fdd1416275233ca3a2472a0c817f598963ab1a8ba7c4c3  vmlinux.symvers' | sha256sum -c -
scratch=$(mktemp -d /tmp/magik-provider-build.XXXXXX)
mkdir "$scratch/source" "$scratch/toolchain" "$scratch/kernel" "$scratch/modules"
tar -xf kernel.tar -C "$scratch/source"
tar -xf toolchain.tar.xz -C "$scratch/toolchain" --strip-components=1
cp -R /provider/scanout-618 /provider/main-window /provider/scanout-slots "$scratch/modules/"
cp kernel.config "$scratch/kernel/.config"
export PATH="$scratch/toolchain/bin:$PATH"
export ARCH=arm CROSS_COMPILE=arm-none-linux-gnueabihf- LOCALVERSION=-MiSTer
export KBUILD_BUILD_TIMESTAMP='Mon Sep 7 17:23:26 CST 2026'
export KBUILD_BUILD_USER=magik-provider KBUILD_BUILD_HOST=isolated-build KBUILD_BUILD_VERSION=1
export KCFLAGS="-fdebug-prefix-map=$scratch=/magik-provider-build -fmacro-prefix-map=$scratch=/magik-provider-build"
arm-none-linux-gnueabihf-gcc --version > "$out/compiler.txt"
make -C "$scratch/source" O="$scratch/kernel" olddefconfig
cmp /inputs/kernel.config "$scratch/kernel/.config"
# Reuse the hash-verified built-in exports from the completed unmodified kernel
# builds window-probe-build-2/3 (identical tables), not fabricated declarations.
make -C "$scratch/source" O="$scratch/kernel" -j8 modules_prepare
cp /inputs/vmlinux.symvers "$scratch/kernel/Module.symvers"
make -C "$scratch/source" O="$scratch/kernel" M="$scratch/modules/scanout-618" modules
cp "$scratch/modules/scanout-618/mister_magik_window_provider.ko" "$out/"
cp /inputs/vmlinux.symvers "$out/"
cp /inputs/kernel.config "$out/"
cd /provider
sha256sum scanout-618/entry.c scanout-618/provider.c scanout-618/provider.h \
    scanout-618/Makefile scanout-618/build-in-container.sh \
    main-window/mister_magik_main_window_layout.h \
    main-window/mister_magik_main_window_policy.h \
    main-window/mister_magik_main_window_uapi.h \
    scanout-slots/mister_magik_scanout_slots_uapi.h > "$out/source-sha256.txt"
modinfo "$out/mister_magik_window_provider.ko" > "$out/modinfo.txt"
arm-none-linux-gnueabihf-nm -u "$out/mister_magik_window_provider.ko" > "$out/imports.txt"
[[ -z "$(modinfo -F depends "$out/mister_magik_window_provider.ko")" ]]
[[ "$(modinfo -F vermagic "$out/mister_magik_window_provider.ko")" == '6.18.38-MiSTer SMP mod_unload ARMv7 p2v8 ' ]]
cd "$out"
sha256sum mister_magik_window_provider.ko vmlinux.symvers kernel.config source-sha256.txt compiler.txt modinfo.txt imports.txt > SHA256SUMS
echo 'Provider builds; entry point intentionally refuses activation. Not a release artifact.'
