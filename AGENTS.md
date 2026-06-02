# AGENTS.md — mister-slint

Operational guide for AI agents (and humans) working on this project. Read this
first. It captures the goal, the hard-won knowledge about how the MiSTer
displays a Linux UI, the tooling we built, and the concrete next steps.

> Keep this file current. When you learn something non-obvious about the MiSTer,
> Slint, or the deploy path, add it here rather than re-deriving it next session.

---

## 1. Goal & current status

**Goal:** run a [Slint](https://slint.dev) UI, via the **Python bindings**, on a
**MiSTer FPGA**, as a simple game/app front end. Long-term it should feel like a
real frontend (own the screen, controller input). Short-term milestone: *show
any Slint UI on the MiSTer's HDMI output*.

**Status (2026-06-02):**

- ✅ Slint Python app builds and runs on desktop (macOS) via `uv`.
- ✅ Self-contained ARM bundle (portable CPython 3.12 + Slint `armv7l` wheel +
  fonts) builds and deploys to the MiSTer.
- ✅ The bundled interpreter runs on-device (`Python 3.12.12`) and Slint
  **renders correctly to `/dev/fb0`** (verified by dumping the framebuffer to
  PNG — see `build/mister-fb*.png`). Software renderer, 1920×1080.
- ✅ **Visible on HDMI** via the Zaparoo boot path (Option A, §7) — we shim
  `zaparoo/frontend` to launch our app, and `MiSTer_Zaparoo` does the
  `video_fb_enable(1)` + tty2 handoff. Persists across reboot.
- ✅ **Stays on screen** via a continuous `animation-tick()` repaint in the UI
  (`ui/app-window.slint`). Without it, fbcon clears `/dev/fb0` to black after
  the first frame (see §8). Trade-off: the continuous software render pegs ~1 CPU
  core (~96%) at ~62 fps and **tears** (Slint's linuxfb path has no vsync — see
  §9). The clean fix for the *persistence* half is to stop fbcon touching the fb
  (see §11).

So: rendering, HDMI routing, and persistence are all working as a milestone.
Remaining work is input, CPU cost, and a cleaner VT/fbcon story.

---

## 2. The MiSTer device (facts)

- Host on the LAN: `192.168.1.117`, SSH `root` / password `1`.
- CPU/OS: ARM Cortex-A9 **armv7l**, minimal Linux, **glibc 2.31**. DE10-Nano,
  **1 GiB DDR3** (≈400 MB free in our tests).
- Stock Python is **3.9.6, stdlib only, no pip** → cannot run Slint (needs
  3.12+). Hence we bundle our own CPython.
- Framebuffer: `/dev/fb0`, driver **`MiSTer_fb`** (`/proc/fb` → `0 MiSTer_fb`),
  **1920×1080×32**, `rgba 8/16,8/8,8/0` → byte order is **B,G,R,X**
  (little-endian). No `/dev/dri` (no DRM/KMS).
- FB mode is set by writing `/sys/module/MiSTer_fb/parameters/mode` as
  `"<fmt> <rb> <width> <height> <stride>"`, e.g. `8888 1 1920 1080 7680`.
- No system fonts, no fontconfig. We bundle DejaVu + a generated `fonts.conf`.
- Relevant `MiSTer.ini` keys observed: `direct_video=1`, `vga_scaler=1`,
  `fb_terminal=1`, `fb_size=0`.
- `MiSTer.ini` has a backup at `/media/fat/MiSTer.ini.bak`.
- `/media/fat` is exFAT (via FUSE). Writing many small files is **slow**; that
  dominates deploy time, not the network.

---

## 3. How the MiSTer puts a Linux UI on HDMI (the important part)

This is the knowledge that took the most effort. Source references are to the
cloned repos under `reference/` (see §6).

### 3.1 Why writing `/dev/fb0` is invisible at the menu

The MiSTer HPS framebuffer has **multiple buffers** spaced `FB_SIZE` apart.
`/dev/fb0` is **buffer 0**. The FPGA scans out **exactly one** buffer to HDMI,
selected by an FPGA SPI command (`UIO_SET_FBUF`). See
`reference/Main_MiSTer/video.cpp:3284` `video_fb_enable(int enable, int n)`:

```c
spi_w(FB_EN | FB_FMT_RxB | FB_FMT_8888); // enable + format
spi_w(fb_addr low); spi_w(fb_addr high); // which buffer (n)
spi_w(fb_width); spi_w(fb_height); ...   // dims + scaled rect + stride
```

At the **main menu**, the code deliberately keeps the FPGA pointed at the
**wallpaper buffer**, not buffer 0:

```c
if (is_menu() && !enable && menu_bg) { enable = 1; n = menu_bgn; } // video.cpp:3293
```

So the Yoshi's-Island wallpaper you see is the menu's wallpaper **buffer
`menu_bgn`**, drawn by the main binary (`draw_*`/`video_menu_bg` in `video.cpp`).
**Buffer 0 (`/dev/fb0`) is never scanned at the menu.** Our Slint app writes
buffer 0 correctly (the PNG dumps prove it) but the FPGA isn't looking at it.

**Empirical confirmation:** we paused the menu process (`kill -STOP $(pidof
MiSTer)`) and the Slint frame *still* didn't appear on HDMI — ruling out simple
"menu paints over us" contention. The framebuffer dump showed Slint; the screen
did not. Only `video_fb_enable(1, 0)` (FPGA SPI, main-binary only) makes buffer
0 visible.

### 3.2 The `main=` boot hook is stock MiSTer

`main=` is a **native** MiSTer.ini option, not a Zaparoo invention:

- `reference/Main_MiSTer/cfg.cpp:141` maps INI key `MAIN` → `cfg.main`.
- `cfg.cpp:612` default is `cfg.main = "MiSTer"`.
- `reference/Main_MiSTer/user_io.cpp:1463` re-execs the configured binary.
- Stock `reference/Main_MiSTer/MiSTer.ini:381` documents `;main=some_binary_file`.

So `main=zaparoo/MiSTer_Zaparoo` makes the stock `/media/fat/MiSTer` re-exec
into the Zaparoo fork binary at boot. Commenting that line (what we did) reverts
to the stock menu and is why Zaparoo stopped launching.

### 3.3 What Zaparoo actually does to own the screen

Zaparoo ships **forks of `Main_MiSTer` and `Menu_MiSTer`** plus a custom menu
FPGA core (`menu_zaparoo.rbf`). The relevant glue is
`reference/Main_MiSTer/support/zaparoo/alt_launcher.cpp`:

1. Detect the frontend by **file existence**: `alt_launcher_configured()` =
   `FileExists("zaparoo/frontend")` (`alt_launcher.cpp:38-47`). (The old
   `ALT_LAUNCHER`/`MENU_RBF` INI knobs were dropped — see `ZAPAROO_FORK.md`.)
2. `spawn()` (`alt_launcher.cpp:340`): fork, switch to **tty2** via `agetty`
   (so the child has a real controlling VT), then:
   - **HDMI path:** `video_chvt(tty2)` + **`video_fb_enable(1)`** → FPGA now
     scans buffer 0 (`/dev/fb0`). The frontend renders there.
   - **CRT path (320×240):** sets fb mode 320×240, gates `status[9]=1`, and the
     frontend copies the top-left 320×240 of `/dev/fb0` into a separate
     FPGA-mapped DDR region at `0x3A000000` (double-buffered) — see
     `reference/zaparoo-frontend/src/app/native_video_writer.cpp`. Requires the
     custom menu core's `rtl/native_video_*.sv`.
3. Loads its custom menu core via `menu_rbf_name()` →
   `"zaparoo/menu_zaparoo.rbf"` (`support/zaparoo/menu_rbf.cpp:5`).
4. Forces `cfg.fb_terminal = 1` (`alt_launcher.cpp:28-34`).

Their on-screen app is a **Qt app using the `linuxfb` QPA plugin**
(`reference/zaparoo-frontend/src/app/main.cpp:78`,
`Q_IMPORT_PLUGIN(QLinuxFbIntegrationPlugin)`) — i.e. the *same* family as our
Slint linuxfb backend. **Rendering tech was never the issue; the fb-enable +
VT handoff is.**

### 3.4 Engineering takeaway

To get Slint on HDMI we need `video_fb_enable(1, 0)` issued (FPGA SPI), the app
on a real VT, and the right fb mode. Only the MiSTer main binary issues that SPI
sequence. So we must either (a) ride Zaparoo's binary, or (b) reproduce the SPI
sequence ourselves, or (c) ship our own `main=` binary. See §7.

**Are we stuck with the Zaparoo _fork_? No.** `video_fb_enable` lives in *stock*
`Main_MiSTer` (`video.cpp:3284`), `main=` is a *stock* `MiSTer.ini` hook, and the
*stock* menu core already drives the HPS framebuffer (the wallpaper uses it). We
borrow Zaparoo's binary only because it's already installed and already calls
those stock APIs; Options B/C replace it with our own code on a bone-stock
MiSTer. The deeper constraint is **rendering**, not the launcher: the MiSTer has
an fbdev with **no DRM/KMS**, so Slint falls back to legacy `linuxfb` and cannot
vsync — that's a Slint limitation independent of who enables the fb (see §9).

---

## 4. Repo layout (this project)

```
pyproject.toml          slint==1.16.1b1; requires-python>=3.12; uv prerelease=allow; dev: paramiko, slint-compiler
.python-version         3.12
ui/app-window.slint     the Hello-World UI (title, counter+button, colour bars)
src/main.py             entry point; MISTER_SLINT_CHECK=1 headless self-test; MISTER_SLINT_SMOKE=1 timed GUI
deploy/mister-slint.sh  MiSTer Scripts-menu entry (execs the bundle's run-mister.sh)
scripts/
  build-arm-bundle.sh   host: assemble build/mister-slint/ (CPython + wheel + fonts)
  deploy_mister.py      host: paramiko deploy WITH live feedback (upload %, extract %, MiSTer load) + prune
  deploy-mister.sh      host: OLD expect-based deploy (superseded by deploy_mister.py)
  run-mister.sh         on-device launcher (env for SLINT linuxfb, fontconfig, LD_LIBRARY_PATH, vmode)
  run-desktop.sh        host: run locally via uv
  mister_ssh.py         host: paramiko helper — run/reboot/reboot-wait/wait/put/get (THE reliable SSH path)
  audit-mister.sh       host: expect-based device audit
  capture-fb.sh         host: expect-based framebuffer capture
  raw_to_png.py         host: convert a /dev/fb0 dump (BGRX) to PNG
reference/              READ-ONLY clones of Zaparoo/MiSTer source (gitignored) — see §6
build/                  gitignored build artifacts + framebuffer PNG dumps
```

On the device, the bundle lives at `/media/fat/mister-slint/` and the Scripts
entry at `/media/fat/Scripts/mister-slint.sh`. Logs: `/tmp/mister-slint.log`.

---

## 5. Workflow & commands

Always go through `uv` on the host; always use **paramiko** (`mister_ssh.py`)
for device comms — see §8 for why `expect`/raw `ssh` was unreliable.

```bash
# Build the ARM bundle (downloads CPython/wheel/fonts once into build/cache/)
scripts/build-arm-bundle.sh

# Deploy with live feedback (upload %, extraction %, MiSTer load avg + free RAM)
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/deploy_mister.py

# Run an arbitrary command / reboot / copy files on the device
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run "uname -a"
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py reboot
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py get /tmp/fb0.raw build/fb.raw

# Reboot and BLOCK until it's actually back (don't blind-sleep). It waits for
# the device to drop off port 22, then polls SSH + `pidof MiSTer` until
# userspace is ready, printing elapsed time. Measured cycle: down ~3s, ready
# ~35s (returns the instant it's up; optional arg = max seconds, default 120).
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py reboot-wait
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py wait   # just poll until ready

# Launch the app over SSH (NOTE: won't show on HDMI — see §3 — but renders to fb0)
... run "pkill -f main.py; nohup /media/fat/mister-slint/run-mister.sh >/tmp/boot 2>&1 & sleep 7; cat /tmp/mister-slint.log"

# Capture what Slint drew (works even when not routed to HDMI):
... run "dd if=/dev/fb0 of=/tmp/fb0.raw bs=1M 2>/dev/null"
... get /tmp/fb0.raw build/fb.raw
uv run python scripts/raw_to_png.py build/fb.raw 1920 1080 build/fb.png

# Local desktop run / self-tests
scripts/run-desktop.sh
MISTER_SLINT_CHECK=1 scripts/run-desktop.sh   # headless property+callback test
```

**Debug trick — see Slint without HDMI routing:** dump `/dev/fb0` and convert to
PNG. This is how we verified rendering. The buffer holds the last frame Slint
drew (Slint only repaints on change, so a static UI is stable).

---

## 6. Reference source (cloned, read-only, gitignored)

Cloned under `reference/` (shallow). These are for study; do not edit/commit.

| Dir | Repo | Why it matters |
|-----|------|----------------|
| `reference/Main_MiSTer` | `ZaparooProject/Main_MiSTer` (fork) | `video.cpp` fb routing (`video_fb_enable` :3284), `main=` hook (`cfg.cpp:141`), and `support/zaparoo/` launcher glue. **`ZAPAROO_FORK.md` is a goldmine** — a hand-written change map of every Zaparoo patch. |
| `reference/Menu_MiSTer` | `ZaparooProject/Menu_MiSTer` (fork) | Custom menu FPGA core (`menu_zaparoo.rbf`); `rtl/native_video_*.sv` is the CRT 320×240 scan-out region. |
| `reference/zaparoo-frontend` | `ZaparooProject/zaparoo-frontend` | Qt+QML+Rust on-screen app. `src/app/main.cpp` (linuxfb QPA), `src/app/native_video_writer.cpp` (CRT DDR writer at `0x3A000000`), `rust/frontend/src/mister_runtime.rs`. |
| `reference/zaparoo-core` | `ZaparooProject/zaparoo-core` | Go service (`MiSTer_Zaparoo`), MiSTer platform integration. |
| `reference/Zaparoo_MiSTer` | `ZaparooProject/Zaparoo_MiSTer` | Just the Downloader DB metadata — installs `zaparoo/frontend`, `MiSTer_Zaparoo`, `menu_zaparoo.rbf`. Confirms file layout. |

Key files to read first: `reference/Main_MiSTer/ZAPAROO_FORK.md`, then
`reference/Main_MiSTer/support/zaparoo/alt_launcher.cpp`, then
`reference/Main_MiSTer/video.cpp` around `video_fb_enable`.

To refresh: `git -C reference/<repo> pull` (or re-clone `--depth 1`).

---

## 7. Paths forward to get Slint on HDMI

Ordered easiest → hardest. **Recommended: Option A.**

### Option A — Piggyback on the installed Zaparoo binary (fastest, low-risk)

The Zaparoo fork (`/media/fat/zaparoo/MiSTer_Zaparoo`) already does *all* the
video plumbing and spawns whatever file is at `zaparoo/frontend` on tty2 with
`video_fb_enable(1)`. So:

1. Back up the real frontend: `mv zaparoo/frontend zaparoo/frontend.real`.
2. Put a small launcher at `zaparoo/frontend` (must remain a regular file so
   `alt_launcher_configured()` stays true) that ignores any `--crt` arg and
   `exec`s our `/media/fat/mister-slint/run-mister.sh`.
3. Re-enable `main=zaparoo/MiSTer_Zaparoo` in `MiSTer.ini` (uncomment line 278).
4. Reboot. `MiSTer_Zaparoo` loads `menu_zaparoo.rbf`, enables the fb, switches
   to tty2, and runs our launcher → Slint should appear on HDMI.
5. **Revert:** restore `zaparoo/frontend`, or re-comment `main=`, then reboot.

Caveats to verify by capture: exact fb dims Zaparoo sets for the HDMI (non-CRT)
path; whether the default CRT-persisted state forces `--crt` (our launcher
should ignore it); that Slint's linuxfb backend is happy on tty2 (it should be —
a real VT also fixes the `VT_GETSTATE ioctl failed` warning we saw over SSH).

### Option B — Reproduce `video_fb_enable(1,0)` ourselves (medium, clean-ish)

Write a tiny ARM helper that issues the same FPGA SPI sequence (port the
`spi_uio_cmd_cont(UIO_SET_FBUF)` + `spi_w(...)` calls from `fpga_io.cpp` /
`video.cpp`), plus `video_chvt`, then launch Slint. Frees us from Zaparoo but
needs the MiSTer ARM toolchain and `/dev/mem` FPGA access, and the *running
core must support the HPS framebuffer* (the stock menu core does). Hardest part
is doing this from the right core/VT context.

### Option C — Ship our own `main=` frontend binary (heaviest)

Fork `Main_MiSTer` minimally so `main=mister-slint/...` boots a stub that sets
up video and launches Slint. Most control, most work, must track upstream.

---

## 8. Lessons learned / gotchas

- **SSH:** use **paramiko with `allow_agent=False, look_for_keys=False`** (see
  `scripts/mister_ssh.py`, modeled on `mister-companion`). Raw `ssh`/`scp` hit
  "Too many authentication failures" (client offers every agent key) and the
  documented MiSTer pubkey-auth hang. `expect` works but is brittle for
  long/streaming commands. **Prefer `mister_ssh.py` for everything.**
- **`/dev/fb0` ≠ HDMI** at the menu (§3). Don't trust "it's in the framebuffer"
  as "it's on screen." Always confirm on the actual HDMI output / by knowing the
  fb-enable state.
- **linuxfb needs a controlling VT.** You'll see `VT_GETSTATE ioctl failed:
  ENOTTY` — even on tty2 under agetty (Slint can't switch the VT to
  `KD_GRAPHICS`). Rendering still works, but the VT stays in text mode.
- **fbcon clobbers `/dev/fb0` → black screen.** Because the VT stays in text
  mode, the kernel framebuffer console (`vtcon1`, `bind=1`) clears `/dev/fb0` to
  black shortly after our first frame. A *static* Slint UI then vanishes.
  **Workaround in place:** a constant `animation-tick()` binding in
  `ui/app-window.slint` forces a repaint every frame so we continuously
  overwrite fbcon. Diagnosis signature: app process alive, `cksum /dev/fb0`
  stuck at the all-black constant. Live = the cksum changes every sample.
- **busybox has no `pkill`.** Use `kill -9 $(pidof python3.12)` to stop the app.
  Under the Zaparoo path that PID *is* `MiSTer_Zaparoo`'s child, so a `-9`
  (treated as a crash) makes it **respawn** the app in ~1s — handy for reloading
  a changed `.slint`/`main.py` without a full reboot (3 crashes → it gives up).
- **libinput quirks DB missing** → `libinput error: ... device quirks` warnings.
  Rendering is fine; if/when we add input, bundle the quirks DB or point
  libinput at one.
- **Fonts:** MiSTer has none. Bundle DejaVu + a minimal `fonts.conf`; set
  `FONTCONFIG_FILE`/`FONTCONFIG_PATH` (done in `run-mister.sh`).
- **exFAT is slow for many small files.** `deploy_mister.py` prunes the bundle
  (CPython `test/`, `idlelib`, `tkinter`, `lib2to3`, `ensurepip`, `pip`,
  `__pycache__`) and shows progress so a slow extract isn't mistaken for a hang.
- **Framebuffer byte order is BGRX** (`rgba 8/16,8/8,8/0`). `raw_to_png.py`
  swaps B/R; keep that in mind for any direct fb work.
- **Don't leave the menu paused.** If you `kill -STOP $(pidof MiSTer)` for an
  experiment, always `kill -CONT` it afterwards (or reboot).
- **Don't blind-sleep on reboot.** The device reboots fast (~35s to userspace,
  drops off the network in ~3s). Use `mister_ssh.py reboot-wait` (or `wait`),
  which detects the down→up transition and returns the instant `pidof MiSTer`
  answers, instead of a fixed `sleep`. Polling port 22 alone is not enough —
  it confirm-runs a command so we don't act before the rootfs is ready.

---

## 9. Performance (rendering)

Instrumented with Slint's `SLINT_DEBUG_PERFORMANCE`. Set `MISTER_SLINT_PERF=1`
and `run-mister.sh` exports `refresh_lazy,console,overlay` → FPS printed to the
log *and* an on-screen FPS overlay (top-left), and it logs the active backend.
The shim sets it so the overlay is visible on HDMI; drop it once tuned.

Measured on-device (Zaparoo HDMI path, 1920×1080, Skia software renderer):

- **~62 fps**, dead steady (`average frames per second: 61–62`).
- **~1 CPU core ~96% busy** (`top`: python ≈ 48% of the dual-core A9 = one full
  core). So frame *count* is not the bottleneck — the renderer keeps up.

**"Not smooth" = tearing, not low fps:**

- The HDMI panel is **60 Hz**, and `MiSTer_fb` *does* support `FBIO_WAITFORVSYNC`
  — measured a rock-steady **16.6 ms** wait (= 60 Hz).
- But Slint renders at **~62 fps**, i.e. it does **not** wait for vsync; it runs
  a ~16 ms software timer and `memcpy`s straight to `/dev/fb0`. 16 ms vs 16.67 ms
  drift means the tear line slowly sweeps the screen → visible judder.
- Slint has **no vsync toggle** for software rendering. Its LinuxKMS backend only
  gets tear-free vsync from **DRM dumb buffers**; with no `/dev/dri` it falls
  back to legacy `linuxfb`, which blits with no vblank sync. Confirmed against
  the Slint docs (LinuxKMS backend) — this is a backend limitation, not config.

**Paths to a tear-free / smoother UI** (none are free; pick when it matters):

1. **Patch Slint's `linuxfb` display** to call `FBIO_WAITFORVSYNC` before each
   present (`internal/backends/linuxkms/display/linuxfb.rs`) and rebuild the
   `armv7l` wheel. Locks to a clean, tear-free 60 fps. Biggest payoff, but
   commits us to building Slint from source for ARM.
2. **Pace frames from Python** at vsync (block on `FBIO_WAITFORVSYNC`, then nudge
   a property). Improves cadence; not guaranteed tear-free since we don't control
   when Slint's blit lands. Cheaper, partial win.
3. **Slow / shrink the motion** so tearing is less noticeable. Cosmetic only.
4. **Don't animate at all** — only viable once fbcon stops clobbering the fb
   (§11); a static frontend has no tearing and ~0 idle CPU.

Repro:

```bash
# With MISTER_SLINT_PERF=1 (shim sets it) → reboot, then:
... run "tail -n 20 /tmp/mister-slint.log"   # average frames per second: NN
... run "top -bn1 | grep python3.12"          # %CPU (≈ one core)
# Prove the fb supports vsync (bundled python, ioctl FBIO_WAITFORVSYNC=0x40044620):
# steady ~16.6 ms waits == 60 Hz available, just unused by Slint's linuxfb path.
```

---

## 10. Current device state & recovery

- `MiSTer.ini` line 278 is `main=zaparoo/MiSTer_Zaparoo` (uncommented) →
  **Zaparoo enabled**: boot → `MiSTer_Zaparoo` → our `zaparoo/frontend` shim →
  `run-mister.sh` → Slint on HDMI. Backup: `/media/fat/MiSTer.ini.bak`. Revert by
  re-commenting that line (or restoring the backup), then reboot.
- `zaparoo/frontend` is our shim (the real Qt frontend is saved as
  `frontend.real`). It exports `MISTER_SLINT_NO_VMODE=1` and — for now —
  `MISTER_SLINT_PERF=1`, so the FPS overlay/logging is on. Drop the PERF line
  when done tuning.
- The mister-slint bundle is deployed at `/media/fat/mister-slint/` and verified
  runnable on HDMI (~62 fps, see §9).
- **Black-screen recovery:** paramiko SSH works even with no usable video. To
  recover from a bad `main=`/frontend experiment: SSH in, re-comment `main=`
  (or restore `MiSTer.ini.bak`) and/or restore `zaparoo/frontend`, then
  `mister_ssh.py reboot`.

---

## 11. Open questions / follow-ups

- **Cleaner fix than the always-on animation** (which pegs a CPU core with
  full-frame 1080p software renders). Options to investigate: get Slint to
  succeed at `KD_GRAPHICS` on tty2 (so fbcon stops touching the fb), or unbind
  fbcon explicitly (`echo 0 > /sys/class/vtconsole/vtcon1/bind`) before/at
  launch, or `setterm`/`KDSETMODE` tricks. If fbcon no longer clobbers `/dev/fb0`
  a static frame would persist and we could drop the heartbeat animation (or
  keep it purely cosmetic). Resolved facts: fb is **1920×1080** full-screen (not
  letterboxed); tty2 under agetty works but `VT_GETSTATE` still fails.
- Input: map controller/keyboard to Slint. Zaparoo maps `JOY_L2/R2/OSD` →
  `F1/Backspace/Menu` (`alt_launcher.cpp:49-66`); we'll need our own scheme and
  an "exit" key (our app currently runs forever with no quit path on-device).
- libinput quirks DB is missing — fine for rendering; revisit when wiring input.
