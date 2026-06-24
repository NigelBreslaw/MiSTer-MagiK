# Production Stream FAT Screenshot Downloads

The runtime screenshot media worker now uses the stream-fat design proven by the
benchmark matrix in `history/2026-6-24-stream-fat-screenshot-download.md`.

Production behavior:

- Fetch the raw identity `.mmlz4b` object with `Accept-Encoding: identity`.
- Stream `wget` stdout directly into a hidden temp beside the final
  `/media/fat/mister-magik/assets/*-screenshots-<size>.mmlz4b` pack.
- Feed the same byte chunks into `sha256sum` or `shasum -a 256`.
- Verify streamed byte count and SHA-256 against both the selected raw variant
  and the pack's raw manifest entry.
- Only after verification, sync the hidden temp, rename it over the final pack,
  sync the parent directory, and update `.screenshot-media-state.json`.
- On any failure before rename, remove the hidden temp and leave the previous
  pack untouched.

This removes the old production sequence of downloading the pack to `/tmp`,
hashing the `/tmp` file, then copying the verified file to `/media/fat`.

## Validation

Host checks:

```text
cargo fmt --manifest-path magik-gui/Cargo.toml --check
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features ui_runner::media_worker
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features artifact_publish
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features media_bench_download
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
scripts/test-host-tools.sh
```

Device checks:

```text
magik-gui/build-arm.sh --device
scripts/deploy-rust.sh
```

Runtime smoke ran the real launcher with
`MISTER_MEDIA_ASSET_DIR=/media/fat/mister-magik/assets-stream-smoke-20260624`
and `MISTER_MEDIA_UPDATE=download`. It downloaded Arcade, NeoGeo, and Saturn
into that temporary FAT directory, emitted `download`, `download_done`,
`verify`, `save`, `sync`, `rename`, `parent-sync`, and `done` progress rows,
reported `screenshot_media_update_done packs=3 current=0 missing=3 stale=0
downloaded=3 failed=0`, and left no `wget`, `sha256sum`, or `shasum` helper
processes behind. The temporary smoke directory was removed afterward.
