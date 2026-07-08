# FPGA Vblank Latch Quartus Docker Attempt - 2026-07-09

## Goal

Build `menu-magik-vblank-latch.rbf` locally on Apple Silicon using OrbStack
Docker with Quartus Prime Lite 17.0 for Linux.

## Setup

- Host: macOS Apple Silicon.
- Docker runtime: OrbStack.
- Quartus installer cache:
  - `QuartusLiteSetup-17.0.0.595-linux.run`
    - SHA1: `99ccfb15962febceba64de2dc9b28c47e5a3b8df`
    - Size: about `2.0G`
  - `cyclonev-17.0.0.595.qdz`
    - SHA1: `2198dedb99866f38d43ff6c029d4bd668e2bbb59`
    - Size: about `1.1G`
- Installed Quartus tree:
  - `build/quartus-lite-17.0/docker-intelFPGA_lite`
  - Size: about `7.9G`
- Quartus version verified inside Docker:
  - `Quartus Prime Shell Version 17.0.0 Build 595 04/25/2017 SJ Lite Edition`

The Quartus installer completes but can leave its wrapper process alive after
the install log says `Installation completed`; the Docker helper treats a
timeout as successful only when that completion marker exists.

OrbStack's amd64 containers report `uname -m=x86_64`, but `/proc/cpuinfo` can
still expose ARM host features. The local installed `qenv.sh` was patched to
bypass only Quartus' shell-wrapper SSE probe. This lets `quartus_sh --version`
and `quartus_map --version` run.

## Runtime Images Tried

- `mister-magik-quartus-runtime:ubuntu20-amd64`
  - Base: `ubuntu:20.04`
- `mister-magik-quartus-runtime:ubuntu18-amd64`
  - Base: `ubuntu:18.04`

Both images can launch Quartus tools.

## Failure

Full compile fails in Analysis & Synthesis:

```text
Info: Command: quartus_map --read_settings_files=on --write_settings_files=off menu -c menu
Error (293007): Current module quartus_map ended unexpectedly.
```

Running `quartus_map` directly shows the real process failure:

```text
realloc(): invalid pointer
qemu: uncaught target signal 6 (Aborted) - core dumped
quartus_map_rc=134
```

With OrbStack `rosetta=true`, the QEMU banner disappears but the abort remains:

```text
realloc(): invalid pointer
quartus_map_rc=134
```

The same failure happens for:

- the MagiK vblank-latched patched Menu_MiSTer work tree, and
- an unmodified stock Menu_MiSTer work tree.

That rules out the MagiK HDL patch as the trigger.

## Conclusion

The local Apple Silicon Docker path is not currently a reliable Quartus 17.0
build environment. It reaches Quartus tool startup, but `quartus_map` aborts
inside translated amd64 execution before producing map reports.

This is an execution-environment blocker, not a vblank-latch source problem.

## Next Recommendation

Build this core on real x86_64 Linux:

1. Use an x86_64 Linux machine or a self-hosted GitHub Actions runner.
2. Reuse the same installer files and `scripts/install-quartus-lite-docker.sh`
   if Docker is preferred there.
3. Run `scripts/build-fpga-vblank-latch-core.sh`.
4. Collect:
   - `build/fpga-vblank-latch/menu-magik-vblank-latch.rbf`
   - `build/fpga-vblank-latch/menu-magik-vblank-latch.metadata.txt`
   - `build/fpga-vblank-latch/menu-magik-vblank-latch.build.log`

GitHub-hosted runners may be tight but not obviously impossible: the installer
files are about `3.1G` together and the installed Quartus tree is about `7.9G`.
Peak disk usage is the main risk, along with the Quartus download/license flow.
