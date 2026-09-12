#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Nigel Breslaw
set -euo pipefail
kernel_revision=6a581bac47c32dfd2525f9874fd263cf08058610
[[ $# == 1 || ($# == 2 && "$1" == --development-trial) ]] || exit 2
trial=0
if [[ $# == 2 ]]; then trial=1; shift; fi
out="$1"
mkdir "$out"
cd /inputs
echo '0702694110b54441b0a8323be43538e1d5b394645c4970616867b56e55496672  kernel.tar' | sha256sum -c -
echo 'd169f9196e3a6c4248ee79ca85987ebce0e4ea9174c1f8d51af9b28fecf22da1  toolchain.tar.xz' | sha256sum -c -
echo '584c7fdb7884616363b38c0514266a5fc40083ae327d9a71e72deb6f3101cdab  kernel.config' | sha256sum -c -
echo 'f58b220d8cdcb925afdd4ba4a4c0a04c02154a8f2fc658cc1fa885b89f79952f  vmlinux.symvers' | sha256sum -c -
scratch=$(mktemp -d /tmp/magik-provider-build.XXXXXX)
mkdir "$scratch/source" "$scratch/toolchain" "$scratch/kernel" "$scratch/modules"
tar -xf kernel.tar -C "$scratch/source"
tar -xf toolchain.tar.xz -C "$scratch/toolchain" --strip-components=1
cp -R /provider/scanout-618 /provider/main-window /provider/scanout-slots "$scratch/modules/"
cp kernel.config "$scratch/kernel/.config"
export PATH="$scratch/toolchain/bin:$PATH"
export ARCH=arm CROSS_COMPILE=arm-none-linux-gnueabihf- LOCALVERSION=-MiSTer
export KBUILD_BUILD_TIMESTAMP='Sat Sep 12 09:57:13 UTC 2026'
export KBUILD_BUILD_USER=magik-provider KBUILD_BUILD_HOST=isolated-build KBUILD_BUILD_VERSION=1
export KCFLAGS="-fdebug-prefix-map=$scratch=/magik-provider-build -fmacro-prefix-map=$scratch=/magik-provider-build"
arm-none-linux-gnueabihf-gcc --version > "$out/compiler.txt"
make -C "$scratch/source" O="$scratch/kernel" olddefconfig
cmp /inputs/kernel.config "$scratch/kernel/.config"
# Reuse the hash-verified built-in exports from the completed unmodified kernel
# builds window-probe-build-2/3 (identical tables), not fabricated declarations.
make -C "$scratch/source" O="$scratch/kernel" -j8 modules_prepare
cp /inputs/vmlinux.symvers "$scratch/kernel/Module.symvers"
make_args=()
module=mister_magik_window_provider.ko
if [[ $trial == 1 ]]; then
  make_args+=(MISTER_MAGIK_DEVELOPMENT_TRIAL=1)
  module=mister_magik_scanout_slots.ko
fi
make -C "$scratch/source" O="$scratch/kernel" M="$scratch/modules/scanout-618" "${make_args[@]}" modules
cp "$scratch/modules/scanout-618/$module" "$out/"
cp /inputs/vmlinux.symvers "$out/"
cp /inputs/kernel.config "$out/"
printf '%s\n' "$kernel_revision" > "$out/kernel-revision.txt"
cd /provider
sha256sum scanout-618/entry.c scanout-618/provider.c scanout-618/provider.h \
    scanout-618/mister_magik_mapping_diagnostic_uapi.h \
    scanout-618/Makefile scanout-618/build-in-container.sh \
    main-window/mister_magik_main_window_layout.h \
    main-window/mister_magik_main_window_policy.h \
    main-window/mister_magik_main_window_uapi.h \
    scanout-slots/mister_magik_scanout_slots_uapi.h > "$out/source-sha256.txt"
modinfo "$out/$module" > "$out/modinfo.txt"
arm-none-linux-gnueabihf-nm -u "$out/$module" > "$out/imports.txt"
[[ -z "$(modinfo -F depends "$out/$module")" ]]
[[ "$(modinfo -F license "$out/$module")" == GPL ]]
[[ "$(modinfo -F mister_magik_source_license "$out/$module")" == GPL-2.0-only ]]
[[ "$(modinfo -F vermagic "$out/$module")" == '6.18.38-MiSTer SMP mod_unload ARMv7 p2v8 ' ]]
if [[ $trial == 1 ]]; then
  [[ "$(modinfo -F mister_magik_development_trial "$out/$module")" == stock-6.18-latch-reuse-v3 ]]
else
  [[ -z "$(modinfo -F mister_magik_development_trial "$out/$module")" ]]
fi
cd "$out"
sha256sum "$module" vmlinux.symvers kernel.config kernel-revision.txt \
    source-sha256.txt compiler.txt modinfo.txt imports.txt > SHA256SUMS
if [[ $trial == 1 ]]; then
  echo 'Development trial provider builds; never use as a release artifact.'
else
  echo 'Provider builds; entry point intentionally refuses activation. Not a release artifact.'
fi
