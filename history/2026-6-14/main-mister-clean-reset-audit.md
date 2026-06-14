# Main_MiSTer Clean Reset Audit - 2026-06-14

## Baseline Compared

Current `main-mister/` was compared against upstream `MiSTer-devel/Main_MiSTer`
commit `c73802332ff9c73659410084b6319ccd29f0b3aa`
(`Release 20260603.`), the fork baseline documented in `main-mister/FORK.md`.

The local upstream reference also contains that release as the newest reachable
`Release YYYYMMDD.` marker. Its checked-out `master` has newer development
commits, but those are not release markers and should not be used as a clean
reset base unless the release policy changes.

Raw diff noise:

- `main-mister/bin/` is build output, not source. It appears as added in a
  no-index diff because upstream release trees do not include local object files.
- `releases/MiSTer_20260603` is an upstream release payload intentionally absent
  from the project fork.

Meaningful source/doc/build changes are limited to:

- `main-mister/FORK.md`
- `main-mister/build-docker.sh`
- `main-mister/cfg.cpp`
- `main-mister/input.cpp`
- `main-mister/main.cpp`
- `main-mister/menu.cpp`
- `main-mister/osd.cpp`
- `main-mister/scheduler.cpp`
- `main-mister/support/mister_magik/alt_launcher.cpp`
- `main-mister/support/mister_magik/alt_launcher.h`
- `main-mister/user_io.cpp`
- `main-mister/video.cpp`
- `main-mister/video.h`

## Current Fork Change Inventory

### Documentation and Build

`FORK.md`

- Documents the upstream baseline, boot model, patch map, and sync rules.
- Keep, but update after a reset to describe the strict launcher ownership
  model instead of the current Zaparoo-style coexistence model.

`build-docker.sh`

- Adds a local Dockerized Main build entrypoint.
- Keep. It is operational tooling, not part of the runtime ownership problem.

### `support/mister_magik/alt_launcher.*`

Current responsibilities:

- Detect whether `/media/fat/mister-magik/mister-magik-fb` exists.
- Apply launcher config side effects (`cfg.fb_terminal = 1`, `cfg.recents = 1`,
  and `s_osd_suppression_intended = true`).
- Write `/tmp/mister_magik_launcher`.
- Spawn Slint through `agetty` on `tty2`.
- Switch VT to tty2.
- Disable Main input/OSD and hide Main menu after spawn.
- Track launcher PID, crashes, respawn timer, deploy lock, bench boot state,
  status JSON, and boot analytics.
- Run Rust `early-black` after `video_init()`.
- Handle `mister_magik_launch` and `mister_magik_exit_to_menu` handoff helpers.

Keep in the clean design:

- Launcher binary detection.
- The post-`video_init()` Rust `early-black` call, unless Slint startup can take
  over this role without visible blank/static time.
- `agetty`/tty2 child spawn, or an equivalent controlled child process launch.
- Status JSON and event log, but rename misleading fields. `visible_owner` based
  on Main's internal `fb_enabled` is not authoritative once Rust owns SPI.
- Deploy-lock-aware spawn deferral.
- Explicit handoff commands for launch and exit.
- Crash policy, but make it a named policy instead of implicit respawn/fallback.

Rewrite or delete:

- `mister_magik_launcher_osd_suppressed()`.
- `s_osd_suppression_intended`.
- `s_stock_menu_enabled`, `s_escaped`, and `s_gave_up` as overlapping booleans.
  Replace with one explicit launcher state enum.
- `stay_in_magik_mode()` and `return_to_normal_mode()` as ad hoc transition
  bundles. Replace with named transitions: `enter_launcher_dormant_mode`,
  `handoff_to_game`, `handoff_to_stock_menu`, `handle_launcher_crash`.
- Respawn-after-clean-exit behavior. Under strict ownership, clean exit should
  mean either an explicit handoff is in progress or a policy violation.
- Bench boot handling inside the production lifecycle module. Keep if still
  useful, but move behind a clearly test-only/developer path.

### `cfg.cpp`

Current change:

- Includes `alt_launcher.h`.
- Calls `mister_magik_launcher_cfg_apply()` at the end of `cfg_parse()`.

Finding:

- Upstream already sets `cfg.fb_terminal = 1` by default in this release.
- The hook mainly forces `cfg.recents = 1` and starts the current OSD
  suppression-intended state.

Clean design:

- Prefer deleting the config hook unless a concrete config value is proven
  necessary.
- If a config hook remains, it must not start UI suppression state. Config parse
  should not be a mode transition.

### `user_io.cpp`

Current changes:

- Includes `alt_launcher.h`.
- After `video_init()`, if running the Menu core and MagiK is configured, runs
  Rust `early-black`.
- In the Menu-core branch, calls `mister_magik_launcher_init_for_menu()` instead
  of `video_menu_bg(...)`.

Keep:

- The placement after `video_init()` is important. Direct `main=mister-magik-fb`
  failed because Slint started before Main initialized HDMI.
- Avoiding `video_menu_bg(...)` on the MagiK launcher path is correct.

Rewrite:

- Replace `mister_magik_launcher_init_for_menu()` with an explicit transition:
  `enter_launcher_dormant_mode_after_menu_init()`.
- That transition should state the invariant: Main is now a boot/command parent,
  not a visible UI owner.

### `scheduler.cpp`

Current changes:

- Calls `mister_magik_launcher_poll()` from the polling cothread.
- Skips `HandleUI()` and `OsdUpdate()` while launcher is active.

Keep:

- Main needs some polling loop to monitor the child and accept handoff commands.

Rewrite:

- Do not rely on scattered `if (!launcher_active)` checks as the main safety
  mechanism.
- Introduce a central dormant scheduler path. While MagiK owns the launcher:
  poll only launcher lifecycle and the minimal command channel needed for
  handoff.
- Extract `/dev/MiSTer_cmd` command processing from full `input_poll()` if full
  input polling can still trigger Main UI/OSD behavior. This is likely the
  cleanest seam for strict ownership.

### `input.cpp`

Current changes:

- Adds FIFO commands:
  - `mister_magik_exit_to_menu`
  - `mister_magik_launch <path>`
- Launch command shuts down Slint, then uses existing `xml_load()` /
  `fpga_load_rbf()` paths.

Keep:

- The explicit command seam is necessary.
- Launching through Main's existing core loader is necessary.

Rewrite:

- Move MagiK command parsing into a narrow command module if possible, so
  dormant mode does not have to run full Main input polling just to receive
  handoff commands.
- Commands should be valid only in compatible launcher states. For example,
  `mister_magik_launch` should fail loudly if MagiK is not active or a handoff
  is already in progress.

### `video.cpp` and `video.h`

Current changes:

- Includes `alt_launcher.h`.
- Suppresses `fb_write_module_params()` while launcher is active.
- Suppresses `video_fb_enable()` while launcher is active.
- Adds analytics around Main framebuffer routing and menu background drawing.
- Adds `video_boot_analytics_snapshot()` and `visible_owner` status support.
- Adds `bg_has_picture` tracking for analytics.

Keep:

- A defensive guard in `video_fb_enable()` may still be useful, but it should be
  treated as an invariant violation, not normal operation.
- Some observability is useful, but it must not pretend Main's internal
  `fb_enabled/fb_num` state is the whole truth after Rust uses SPI.

Delete or rewrite:

- The normal-path `video_fb_enable_suppressed` behavior. In the clean design,
  `video_fb_enable()` should not be reached while MagiK owns the launcher.
- Menu background analytics unless needed for a specific test.
- `video_boot_analytics_snapshot()` as currently named, or at least rename its
  output to `main_fb_state` instead of `visible_owner`.
- `fb_write_module_params_suppressed` as normal behavior. Dormant mode should
  prevent the caller, not depend on this callee refusing work.

### `osd.cpp`

Current changes:

- Suppresses `OsdEnable()`, `InfoEnable()`, `OsdMenuCtl()`, and `OsdUpdate()`
  while MagiK launcher suppression is active.
- Tracks suppression counts.

Clean design:

- Delete as normal operating logic.
- Optionally keep a tiny defensive assertion/log in OSD entry points:
  if launcher dormant mode is active and OSD is called, disable OSD and record an
  `unexpected_osd_call` event.
- The scheduler/state machine should prevent OSD work from being scheduled in
  the first place.

### `menu.cpp`

Current changes:

- Adds analytics around `MenuHide()`.

Clean design:

- Delete unless needed for a specific transition test.
- Main should not be repeatedly hiding its menu while MagiK owns the launcher.
  It should enter dormant mode once.

### `main.cpp`

Current changes:

- Adds boot analytics around start and after `user_io_init()`.

Clean design:

- Optional. Keep if boot analytics remain useful, but not required for strict
  ownership.

## Minimal Clean Patch Set After Reset

Starting from upstream release `c7380233` or a newer release marker, reapply only
these runtime concepts:

1. **Launcher mode module**

   Add a MagiK module with one explicit state machine:

   - `Unconfigured`
   - `BootingMain`
   - `EnteringLauncher`
   - `LauncherActive`
   - `HandoffToGame`
   - `HandoffToStockMenu`
   - `LauncherCrashed`

   Avoid overlapping booleans for escaped/gave-up/stock-menu/suppression.

2. **Post-video-init boot hook**

   After `video_init()` and only for the Menu core with MagiK configured:

   - run Rust `early-black` or equivalent;
   - enter `LauncherActive`/dormant mode;
   - spawn Slint on tty2;
   - do not draw Main menu background.

3. **Dormant Main scheduler**

   While `LauncherActive`:

   - do not call `HandleUI()`;
   - do not call `OsdUpdate()`;
   - do not call `video_menu_bg()`;
   - do not call `video_fb_enable()` except as an explicit handoff;
   - do not run broad input paths that can open Main UI;
   - do poll child lifecycle and a minimal command channel.

4. **Command handoff seam**

   Keep FIFO commands, but route them through state transitions:

   - `mister_magik_launch <path>` -> stop Slint -> leave dormant mode -> launch
     through `xml_load()` / `fpga_load_rbf()`.
   - `mister_magik_exit_to_menu` -> stop Slint -> leave dormant mode -> restore
     stock Main menu behavior.

5. **Crash policy**

   Decide and implement one policy:

   - respawn Slint and keep Main dormant; or
   - intentionally fall back to stock Main menu.

   Do not silently mix both. Status must say which policy fired.

6. **Defensive assertions**

   Keep small guards in dangerous entry points if needed, but make them report
   invariant violations:

   - `unexpected_video_fb_enable_while_launcher_active`
   - `unexpected_osd_call_while_launcher_active`
   - `unexpected_menu_bg_while_launcher_active`

   These should be test failures or diagnostics, not ordinary expected logs.

7. **Status and diagnostics**

   Keep status JSON, but split facts clearly:

   - `launcher_state`
   - `launcher_pid`
   - `active_vt`
   - `main_fb_state` from Main bookkeeping
   - `rust_fb_route_last_reasserted` or equivalent from Slint/Rust

   Do not call Main bookkeeping `visible_owner` unless it is backed by an
   authoritative live FPGA read.

## Changes Expected To Go Away

These are artifacts of the old clever coexistence/suppression model:

- OSD suppression hooks in `osd.cpp`.
- `mister_magik_launcher_osd_suppressed()`.
- `s_osd_suppression_intended` and `mister_magik_note_osd_suppressed()`.
- Normal-path `video_fb_enable_suppressed` and
  `fb_write_module_params_suppressed`.
- Menu background instrumentation in `video_menu_bg()` unless retained only for
  a one-off device test.
- `MenuHide()` analytics.
- `visible_owner` derived only from Main's `fb_enabled/fb_num`.
- Clean-exit auto-respawn behavior.
- `stay_in_magik_mode()` / `return_to_normal_mode()` as bundled side-effect
  helpers.
- Config parse as a launcher mode transition.

## Test Plan For The Clean Reapply

### Host tests

1. **Patch surface test**

   Add a script that compares `main-mister/` to the selected upstream release
   and fails if source changes appear outside the approved patch map.

2. **State machine tests**

   Extract the launcher state transitions into a small testable module. Test:

   - boot -> launcher active;
   - launch command -> handoff to game;
   - exit command -> handoff to stock menu;
   - crash -> selected crash policy;
   - duplicate command during handoff is rejected.

3. **Command parser tests**

   Test safe parsing of `mister_magik_launch <absolute path>` and
   `mister_magik_exit_to_menu`, including malformed/relative paths.

4. **Status contract tests**

   Test that status fields distinguish launcher state, Main framebuffer
   bookkeeping, and Rust framebuffer route data.

### Device tests

1. **Boot invariant**

   Reboot into `main=MiSTer_MagiK`. Assert:

   - Slint process exists;
   - active VT is `tty2`;
   - status says `launcher_state=LauncherActive`;
   - no `unexpected_*_while_launcher_active` events occurred;
   - framebuffer snapshot is Slint-like.

2. **No Main UI while active**

   Let the launcher idle for several minutes. Assert no Main OSD/menu/background
   events fire in normal operation.

3. **Game handoff**

   Send `mister_magik_launch <known .mgl/.mra>`. Assert:

   - Slint stops;
   - launcher state leaves active mode;
   - Main launches the expected core;
   - no respawn occurs during game launch.

4. **Exit to stock menu**

   Send `mister_magik_exit_to_menu`. Assert:

   - Slint stops;
   - stock Main menu appears;
   - OSD/menu input works normally after handoff.

5. **Crash policy**

   Kill Slint unexpectedly. Assert the selected crash policy occurs and is
   reflected in status and events.

6. **Deploy lock**

   Create the deploy lock while Main is active. Assert Main does not spawn a
   half-updated launcher, then spawns after lock removal.

## Recommendation

Do reset to a public release and reapply. But do not reapply the current patch
map mechanically.

The new seam should be **LauncherActive dormant Main**, not **Main continues to
run with scattered suppressions**. The difference is testability: in the clean
model, any Main UI/framebuffer call during launcher active mode is an invariant
violation. In the current model, those calls are expected to happen and are then
suppressed locally, which leaves too many places for old Main behavior to leak
back onto HDMI.
