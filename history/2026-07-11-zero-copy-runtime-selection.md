# Zero-copy runtime selection fix

## Confirmed cause

The renderer selected Slint `SwappedBuffers` only from the obsolete
`MISTER_TRUE_ZERO_COPY=1` environment variable. The production scanout parser
already canonicalizes required/auto/legacy mode, so production configuration
could enable the atomic backend while Slint incorrectly remained on
`ReusedBuffer`.

## Before

- Canonical required mode: atomic backend requested, `ReusedBuffer` selected.
- Real swapped-buffer zero-copy frames: 0.

## After

- Canonical required mode selects `SwappedBuffers`.
- Device smoke reached 34 real latch-backed frames and reported zero buffer
  alternation failures before the separately tracked completion-fence stall.

Evidence:

- `build/launcher-home-scroll-profiles/PROD-ZC-ACP-FIX-20260711-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PROD-ZC-ACP-FIX-20260711-launcher-home-scroll.log`

Validation:

- 244 host logic tests.
- Rust checks and clippy with warnings denied through the repository pre-commit
  gate.
- ARM `release-device` build with `ui,bench-tools,diagnostics`.
