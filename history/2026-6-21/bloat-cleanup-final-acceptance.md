# Bloat Cleanup Final Acceptance

Date: 2026-06-21

This records final validation for the bloat cleanup commit series that locked
the production path to RGB565 runtime, fixed preview fade, minimal command
surface, runtime-only deploy, fixed catalog/media artifacts, and experiment-only
visual galleries.

## Host Validation

Passed:

- `scripts/dev-rust test`
- `scripts/dev-rust check`
- `scripts/dev-rust host-tools`
- `scripts/profile-preview-scroll.sh --self-test`
- `scripts/bench-toolchain.sh --self-test`
- `cargo test --manifest-path tools/mister/Cargo.toml`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features`

## Device Runtime Deploy

Passed:

- `scripts/deploy-rust.sh`

Result:

- Built `release-device` with `features=ui`.
- Deployed `/media/fat/mister-magik/mister-magik-fb` through the agent.
- Binary size: `5701364` bytes.
- Launcher resumed under `MiSTer_MagiK`.
- Catalog was ready with `7221` games after deploy.

## Device Release Acceptance

Failed:

- `scripts/device-release-acceptance.sh`

Artifacts:

- `build/device-release/20260621T190427Z/report.md`
- `build/device-release/20260621T190427Z/`

Summary:

- Health, framebuffer-route, launcher lifecycle, supervised reboot soak, crash
  recovery, first-boot scan UI, exit-to-menu handoff, and game handoff checks
  mostly passed.
- The run reported `device release acceptance: FAIL (9 failures)`.
- Failed checks:
  - `audio-tone /dev/MrAudio probe`
  - `launcher_catalog table count expected=1 actual=empty`
  - `runtime-only screenshot asset table count expected=0 actual=empty`
  - `arcade has_preview count expected > 0 actual=empty`
  - `neogeo has_preview count expected > 0 actual=empty`
  - `saturn has_preview count expected > 0 actual=empty`
  - `catalog mutation fixture count expected growth first=0 second=0`
  - `wait-launcher-after-display-1080p`
  - `wait-launcher-after-install-roundtrip`
- After the failure, `scripts/mister agent magik restart-launcher` recovered the
  launcher without manual reboot.

## Device Catalog Acceptance

Failed:

- `scripts/device-catalog-acceptance.sh`

Observed output before exit:

- `launcher process count = 1`
- `active library-refresh count = 0`
- `/media/fat/mister-magik/library.sqlite3 is present and non-empty`

The command exited with status `2`.

## Device Preview Evidence

Passed:

- `scripts/profile-preview-scroll.sh 60 held-scroll BLOATCLEAN-FINAL --skip-build --visual-captures 0`

Key result:

- `frames_after_30=3568`
- `p99_work_us=13876`
- `work_gt_16_7ms=0`
- `vsync_source_vsync=3568`
- `vsync_source_fallback=0`
- `max_vsync_miss_streak=0`

Passed:

- `scripts/gate-preview-60fps.sh BLOATCLEAN-FINAL-GATE --skip-build --visual-captures 0`

Gate results:

- Held-scroll: `p99_work_us=13921`, `work_gt_16667=0`,
  `vsync_source_fallback=0`, `max_miss_streak=0`
- Turbo-hold: `p99_work_us=13969`, `work_gt_16667=0`,
  `vsync_source_fallback=0`, `max_miss_streak=0`

## Follow-Up

The cleanup series is host-clean and the preview 60fps gate passes, but final
device acceptance is not green. Follow-up should focus on the release/catalog
acceptance failures above, especially the empty SQL/projection results and the
display/install-roundtrip recovery waits.
