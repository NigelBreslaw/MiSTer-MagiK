# UiBootFramebufferSession Metrics

Parent: `1fc71f4f7932bdb0c4cb084aed5745d4e874f383`

Benchmark:

- BEFORE: `scripts/profile-warm-catalog-start.sh ARCH-BOOT-BEFORE --replace-label --iterations 5`
- AFTER: `scripts/profile-warm-catalog-start.sh ARCH-BOOT-AFTER --replace-label --iterations 5`

Named metrics:

- `first_frame_ms`: BEFORE samples `35,34,47,47,35`; AFTER samples `47,35,35,34,42`.
  Median stayed `35 ms`; max stayed `47 ms`; no meaningful regression.
- `first_frame_catalog_ready`: all BEFORE/AFTER samples reported `true`.
- Display fallback route regressions: none observed in benchmark status; all samples ended `ok`.

Evidence rows:

- `history/toolchain-bench/results-warm-catalog.tsv`
  - `ARCH-BOOT-BEFORE` rows 1-5
  - `ARCH-BOOT-AFTER` rows 1-5
- `build/warm-catalog/ARCH-BOOT-BEFORE-1.log`
- `build/warm-catalog/ARCH-BOOT-BEFORE-2.log`
- `build/warm-catalog/ARCH-BOOT-BEFORE-3.log`
- `build/warm-catalog/ARCH-BOOT-BEFORE-4.log`
- `build/warm-catalog/ARCH-BOOT-BEFORE-5.log`
- `build/warm-catalog/ARCH-BOOT-AFTER-1.log`
- `build/warm-catalog/ARCH-BOOT-AFTER-2.log`
- `build/warm-catalog/ARCH-BOOT-AFTER-3.log`
- `build/warm-catalog/ARCH-BOOT-AFTER-4.log`
- `build/warm-catalog/ARCH-BOOT-AFTER-5.log`

Commands run:

- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features ui_runner::ui_boot`
- `scripts/dev-rust test`
- `scripts/dev-rust check`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `cargo check --manifest-path magik-gui/Cargo.toml --features ui,experiments --no-default-features`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui,experiments --no-default-features ui_runner::ui_boot`
- `git diff --check`

Additional diagnostic check:

- `cargo check --manifest-path magik-gui/Cargo.toml --features ui,diagnostics --no-default-features`
  still fails on pre-existing diagnostics dead-code warnings outside this item
  (`artifact_publish`, `launcher`, `media_pack_save`, and other existing code).
  The item-introduced dead-code finding from review was fixed: the old
  `Fpga::fb_enable` and `Fpga::fb_enable_direct` wrappers were removed, and
  `FpgaFramebufferRoute::with_offsets` is gated with its experiment-only use.
