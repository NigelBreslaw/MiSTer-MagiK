# Cross-build profiles (`mister-magic-fb`)

Two **release** profiles separate fast host compiles from the binary we ship to the MiSTer.

| Profile | Command | LTO | CGUs | ARM flags | Clean build (~) | Binary (~) | Use |
|---------|---------|-----|------|-----------|-----------------|------------|-----|
| **`release`** | `build-arm.sh` or `--fast` | thin (`lto = true`) | 16 (default) | generic armv7 | ~3 min | ~1.65 MB | Daily Slint/UI iteration, quick deploy |
| **`release-device`** | `build-arm.sh --device` | fat | 1 | cortex-a9 + NEON | ~5 min | ~1.61 MB | SD card / bench / production |

Benchmark labels: **A0** ≈ `release`, **A3** ≈ `release-device` (see [`history/toolchain-bench/`](../history/toolchain-bench/)).

## Commands

```bash
# Fast — default for bare build-arm.sh
rust/build-arm.sh
# → target/armv7-unknown-linux-gnueabihf/release/mister-magic-fb

# Full MiSTer release (fat LTO + NEON via RUSTFLAGS)
rust/build-arm.sh --device
# → target/.../release-device/mister-magic-fb

# Deploy (default = release-device)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh

# Deploy after a fast build (same path on device, larger binary)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --fast
```

`scripts/bench-toolchain.sh` calls `build-arm.sh` with no flags → **`release`** (matches historical A0 toolchain experiments unless you edit the script to pass `--device` for A3-style benches).

## Config files

- **`Cargo.toml`** — `[profile.release]` vs `[profile.release-device]` (inherits release, overrides LTO/CGU).
- **`.cargo/config.toml`** — sccache override only; no always-on `rustflags`.
- **`build-arm.sh`** — sets `RUSTFLAGS` for `release-device` only.

Prerequisite for NEON: `scripts/audit-mister.sh` → `A1 prerequisite: OK`.
