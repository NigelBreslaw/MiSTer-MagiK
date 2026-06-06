# Whole-Boot Visual Analytics

Date: 2026-06-07

## Capture

Bundle:

```text
build/boot-analytics/20260606T215850Z
```

Command:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/boot-analytics.sh --deploy
```

The bundle includes `boot-analytics.tsv`, `boot-summary.md`,
`launcher-frame-profile.tsv`, `visual-samples.tsv`, `slint.log`, `main.log`, and
`device-state.txt`.

## What The Capture Shows

- `MiSTer_MagiK` starts at boot `4062ms`. The static visible before this is
  outside the fork's process lifetime, so it is most likely the stock MiSTer
  pre-`main=` handoff or HDMI/core initialization, not the Slint child.
- From `4543ms` to `5002ms`, Main reports the visible owner as `core`.
- At `5114ms`, Main routes HDMI to `fb0` (`video_fb_enable_on`).
- `/dev/fb0` is still completely black when Slint opens it at `5920ms`, and
  remains black after Slint reasserts the route at `5930ms`.
- Slint does not copy its first frame until `7420ms`. That first copy changes
  the framebuffer classification from `mostly_black` to `slint_like`.
- The first stable sample at frame 30 is still `slint_like` with the same sampled
  hash, so the frame is stable after first copy.
- No `video_menu_bg_done` events were captured inside the MagiK fork, so the
  visible static is not coming from MagiK's menu background drawing after the
  fork starts.
- Main still attempts OSD updates while Slint runs, but they are suppressed and
  do not change the sampled framebuffer.

## Timing Breakdown

- Main start to launcher spawn: `785ms`.
- Spawn to Main routing `fb0`: `267ms`.
- Main routes `fb0` to Slint process start: `636ms`.
- Slint process start to `run_ui_start`: `110ms`.
- `run_ui_start` to first frame: `1580ms`.
- `app_show` to first render: `1380ms`.
- First render to first copy: `30ms`.
- First copy to stable frame 30: `520ms`.

The dominant black-screen cause is not display setup. It is that Main routes
black `/dev/fb0` at `5114ms`, but Slint waits until after the cached arcade
catalog load (`~1.24s`) before first rendering/copying at `7420ms`.

## Proposal For Clean Fast Boot

1. Keep the current Main OSD suppression.
2. Stop showing an unpainted framebuffer:
   - either delay Main's `video_fb_enable(1, 0)` until Slint has copied its
     first frame, or
   - have Slint paint/copy a minimal splash/launcher shell before catalog load.
3. Prefer the second option first: paint a cheap branded shell immediately after
   `app_show`, then load the catalog and update the populated launcher.
   This should reduce black screen by about `1.2-1.4s` without changing the
   Main handoff timing.
4. For the earlier static, the cleanest fix is architectural: skip the stock
   pre-`main=` visual path by making `/media/fat/MiSTer_MagiK` the inittab boot
   target, or add an earlier stock-MiSTer-compatible black/splash route before
   the `main=` reexec. The current update_all-compatible handoff cannot fully
   hide visuals emitted by the unmodified stock `/media/fat/MiSTer` before it
   reexecs the fork.

