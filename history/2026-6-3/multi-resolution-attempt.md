# Multi-resolution attempt — what changed and why it probably failed

**Status:** Reverted. Working tree is back to `61a8fe8` (“Use slint master”): fixed
1920×1080, `fb` / `ui` with explicit `xoff=yoff=0`, no `video_config.rs`.

**Git:** The attempt lived in commits `0a5046b` (“Don't just support 1080p”) and
`588ee26` (“Resolution test”). They are not on `main` anymore after the revert.
You can still inspect them with `git show 0a5046b` / `git show 588ee26`.

This note is a post-mortem from the agent session that broke HDMI for the user
(vertical colour bars, wrong patches, captures showing login text). It is **not**
a design doc — mostly guesses because a correct mental model would not have made
things worse.

---

## What worked before the attempt

At `61a8fe8`:

- `fb` painted a 1920×1080 quadrant test into `/dev/fb0` with **compact** row
  layout (`buf[y * W + x]`).
- `fb_enable_direct(..., Some(0), Some(0))` — hard-coded offsets, not INI-derived.
- `ui` used the same: `MODE_1080P60`, 1920×1080, offsets `0, 0`.
- No sysfs mode writes, no `video_mode` preset table in Rust.
- Slint path was already proven (see `history/2026-5-2/framebuffer-experiments.md`).

---

## What we added (commits `0a5046b` + `588ee26`)

### New module `rust/src/video_config.rs` (~500 lines)

- Copied `vmodes[0..14]` timing table from Main_MiSTer `video.cpp`.
- Parsed `/media/fat/MiSTer.ini` via `ini-preserve` (with a scrape fallback).
- `resolve` / `resolve_for_device` / `resolve_for_preset` to produce `FbGeometry`:
  fb size, `hact`×`vact` for SET_FBUF scaled window, `xoff`/`yoff`, `direct_video`, etc.
- `apply_sysfs_mode` — `echo 8888 1 W H stride > .../MiSTer_fb/parameters/mode`.
- `read_live_fb_size` via `UIO_GET_FB_PAR` when the stock menu was SIGSTOPped.
- Helpers: aspect buckets, `center_in_hdmi`, `set_video_mode_preset` (unused on device).

### `rust/src/main.rs`

- `fb` no longer fixed 1920×1080; called `resolve_for_device` + `fb_test_geom`.
- New subcommands: `fb preset <0-14>`, `fb-matrix` (cycle 8, 4, 6, 1 without reboot).
- `read` also printed INI-resolved geometry.
- `fill_rect` still used **compact** indexing in the committed version (same as before).

### `rust/src/ui_runner.rs`

- `ui` sized the window and fb from `resolve_for_device` instead of constants
  `1920×1080` + `Some(0), Some(0)`.

### `rust/src/fpga.rs`

- `fb_enable_direct` took required `xoff`/`yoff` instead of `Option` with table
  porch defaults — callers were expected to supply resolved offsets.
- Still sent `stride = fb_width * 4` to SET_FBUF (same as Main_MiSTer).

### `rust/src/fb.rs`

- Renamed `FB_SIZE_PX` → `FB_SLOT_PIXELS` via `video_config` (no stride logic in
  the committed commits).

### Scripts

- `scripts/run-resolution-visual-tests.sh` — SIGSTOP once, run `fb-matrix`.
- `scripts/run-resolution-reboot-tests.sh` — edit INI `video_mode`, reboot per preset.

### Other

- `ini-preserve` in `Cargo.toml`, small `rust/build-arm.sh` RUSTFLAGS scoping,
  AGENTS.md notes about multi-resolution (later reverted with the code).

---

## Extra changes tried in the same session (mostly uncommitted)

These were piled on after vertical bars were already reported; they may never have
been built/deployed successfully (deploy was interrupted).

| Change | Intent (as stated at the time) |
|--------|----------------------------------|
| `FBIOGET_FSCREENINFO` / `line_length` in `fb.rs` | Paint and dirty-row copy using driver stride, not `w` |
| Pass `line_length` to SET_FBUF instead of `fb_w*4` | Match sysfs padding if any |
| `geom.stride` left at **0** in `apply_sysfs_mode` | Bug: comment said “fill later” but nothing did |
| `ini_get_first` for `direct_video` / `video_mode` | MiSTer.ini repeats keys; scrape used last-wins |
| `menu_live` heuristic for `hact`/`vact` | If live fb size ≠ preset expected size, use fb size as viewport |
| `center_in_hdmi` in `fb-matrix` | Center smaller presets inside boot HDMI frame |
| `set_vga_fb` + `UIO_BUT_SW` / `CONF_VGA_FB` | Port tail of `video_fb_enable` when `direct_video` |
| `capture-fb.sh` / `raw_to_png.py` stride arg | PNG from padded fb dumps |

---

## What the user saw

- **HDMI:** solid **vertical colour bars** (R/G/B/Y stripes), not the 4-quadrant test.
- Sometimes a **small coloured patch** (wrong placement / scale).
- **`/dev/fb0` PNG captures** often showed **Linux login** after the test ended —
  not proof of what HDMI showed mid-test. One mid-test capture reportedly still
  had vertical stripes in the buffer.
- **Slint** (`mister-fb4.png` from earlier) could look fine in the dump while `fb`
  looked wrong on HDMI — suggests the regression was in the **geometry / fb path**,
  not necessarily Slint rendering itself (guess).

---

## Guesses about why it failed (low confidence)

1. **Three different “resolutions” conflated**  
   We mixed: INI `video_mode` preset, live `GET_FB_PAR` fb_w/h, and `GET_VRES`
   (direct_video scan-out size). Main_MiSTer’s `video_fb_enable` uses **`v_cur.item[1/5]`**
   for the scaled window, not necessarily the INI preset table or GET_VRES. We
   may have sent SET_FBUF dimensions that did not match what the FPGA/menu actually
   had programmed.

2. **Changing fb size without changing HDMI mode**  
   `fb-matrix` / `fb preset` resized `/dev/fb0` and SET_FBUF inside the **boot**
   HDMI timing. User testing showed smaller presets as a corner patch on 1080p —
   then “fixes” may have made the mismatch worse (stretch / stride shear).

3. **Sysfs mode with stride 0** (uncommitted bug)  
   `apply_sysfs_mode` used `geom.stride` which was always 0 in `resolve_preset`.
   If that reached the device, the driver’s `line_length` might not match how we
   painted or what we told the FPGA — classic cause of **vertical colour columns**.

4. **Stride / indexing mismatch** (partial fix that never shipped cleanly)  
   Vertical bars often mean “read memory as width A, laid out as width B”. We
   toggled between compact `w`, `line_length`, and `fb_w*4` in different places
   without one verified end-to-end capture **during** the 20s hold.

5. **`set_vga_fb` might matter; might not**  
   Main_MiSTer calls `set_vga_fb(enable)` after SET_FBUF when `direct_video` — it
   sets `CONF_VGA_FB` via `UIO_BUT_SW`. We never proved whether missing that caused
   the bars or only affected a different output path. It was a late guess.

6. **INI parsing still wrong in places**  
   Duplicate `direct_video=0` lines after `=1` confused early logic; “first key wins”
   scraping was added late. Wrong `direct_video` flag → wrong `xoff`/`yoff` defaults
   in `video_config` (porch formula vs `0,0`).

7. **Operational noise**  
   Reboot test script interrupted → `video_mode` left on a non-8 preset while the
   user expected 1080p. SIGSTOP/CONT and fbcon can overwrite `/dev/fb0` with login
   after exit — easy to misread captures.

---

## What we should have done instead (hindsight, still tentative)

- Keep the proven path: **1920×1080, `xoff=yoff=0`, stride `7680`**, one command.
- Any multi-resolution work should start from **reading live `v_cur` equivalent** on
  device (or only support modes after a full menu reboot), with HDMI verified each step.
- One variable per experiment: sysfs only, or SET_FBUF only, never both + INI + preset
  sweep in one go.
- Capture **during** the hold window; compare dump to HDMI in the same second.

---

## Recovery pointer

```bash
# Current good baseline
git checkout 61a8fe8

# Inspect the reverted attempt only
git show 0a5046b --stat
git show 588ee26 --stat
```

Deploy and run (unchanged from AGENTS.md):

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run \
  'MP=$(pidof MiSTer); kill -STOP $MP; /media/fat/mister-magic/mister-magic-fb fb; kill -CONT $MP'
```
