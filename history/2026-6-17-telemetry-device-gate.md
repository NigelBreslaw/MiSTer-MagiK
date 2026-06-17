# Telemetry Device Gate Expansion - 2026-06-17

## Summary

The device acceptance gate was expanded to prefer observable device state over
blind sleeps. The launcher now writes richer `/tmp/mister-magik/status.json`
telemetry, and `scripts/device-release-acceptance.sh` waits on status JSON,
event names, and trace row growth for the new checks.

The long soak remains opt-in behind `--soak`.

## Baseline Commands

- `scripts/release-check-host.sh` passed.
- `scripts/device-catalog-acceptance.sh --race-refresh` passed.
- `scripts/device-release-acceptance.sh --skip-deploy --fast` passed the new
  telemetry-backed route, preview, velocity-scroll, controller, and audio checks
  before finding the known crash-restart blocker.

## Confirmed Telemetry

- `catalog_scan_visible`, `catalog_scan_title`, `catalog_scan_detail`, and
  `catalog_scan_percent`
- `arcade_selected`, `arcade_visual_index`, `preview_cache_state`,
  `preview_transition_effect`, and `preview_transition_progress`
- `bench_scenario`, `start_screen`, and `lock_screen`
- `route_reassert_count`, `last_route_reassert_frame`,
  `last_route_reassert_ok`, and `last_route_reassert_error`
- `input_pad_count`, `active_pad_index`, `active_pad_name`,
  `active_pad_path`, `last_raw_event`, and `last_input_ms_ago`
- `rss_kb`, `rss_hwm_kb`, `last_frame_ms_ago`, and rolling frame timings

## Findings From The Run

- Framebuffer route contamination initially failed: route events were emitted,
  but the framebuffer remained mostly black. The fix was to force a full present
  on every scheduled route reassert, not only on initial route ownership.
- Crash-policy restart remains a release blocker. Killing `mister-magik-fb`
  moves Main to `LauncherCrashed`, but `mister_magik_restart_launcher` does not
  return to `LauncherActive`.
- Supervised reboot did not recover the crash state in the observed run. Raw
  reboot did recover the device.

## Artifact Pointers

- First route-recovery failure: `build/device-release/20260617T152436Z/`
- Telemetry-expanded fast run: `build/device-release/20260617T153044Z/`
- Local baseline note: `build/device-telemetry-baseline/20260617T153900Z/`

## Fix Plan

1. Fix Main crash restart recovery.
   - Reproduce by killing the supervised `mister-magik-fb` child and sending
     `mister_magik_restart_launcher`.
   - Expected result: Main leaves `LauncherCrashed`, spawns a fresh launcher
     child, restores `tty2`/framebuffer ownership, and reports
     `LauncherActive`.
   - Add or expose `last_crash_reason`, `crash_count`, and
     `last_restart_error` in `main-status.json` if the failure is not obvious
     from existing logs.

2. Fix supervised reboot from `LauncherCrashed`.
   - The observed supervised reboot path did not drop the device or recover the
     launcher after the crash state.
   - Expected result: `scripts/mister reboot-wait` works from
     `LauncherActive`, `LauncherCrashed`, and handoff states, with raw reboot
     reserved for deliberate recovery tests.

3. Refine input age telemetry.
   - `last_raw_event` and controller identity are now exposed, but
     `last_input_ms_ago` should be backed by a real timestamp of the most recent
     raw input transition rather than a coarse presence flag.

4. Run the full default gate after crash recovery is fixed.
   - Command: `scripts/device-release-acceptance.sh --skip-deploy`
   - Then run the explicit soak: `scripts/device-release-acceptance.sh
     --skip-deploy --soak`.
