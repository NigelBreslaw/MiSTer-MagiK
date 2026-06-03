# AGENTS.md — mister-slint

Operational guide for AI agents (and humans) working on this project. Read this
first. It captures the goal, the hard-won knowledge about how the MiSTer
displays a Linux UI, the tooling we built, and the concrete next steps.

> Keep this file current. When you learn something non-obvious about the MiSTer,
> Slint, or the deploy path, add it here rather than re-deriving it next session.

---

## 1. Goal & current status

**Goal:** run a [Slint](https://slint.dev) UI on a **MiSTer FPGA** as a simple
game/app front end. Long-term it should feel like a real frontend (own the
screen, controller input).

**Status (2026-06-03):**

- ✅ **Native Rust frontend renders Slint at a locked 60fps, smooth + tear-free,
  fully fork-free** (no Zaparoo) — `mister-magic-fb ui`. See §9.7 for the
  architecture and the write-combining finding that ruled out FPGA page-flipping.
- ✅ FPGA SPI + `video_fb_enable(1,0)` ported in Rust (`rust/src/fpga.rs`).
- ✅ Custom Slint `Platform` + dirty-row copy into write-combined `/dev/fb0`
  (`rust/src/fb.rs`, `rust/src/main.rs`).

The Python/Slint bundle and Zaparoo shim were removed (2026-06-03); they were
the exploration path. See `history/2026-5-2/framebuffer-experiments.md`.

Remaining work: input, live-mode geometry, shipping as a `main=` boot binary.

---

## 2. The MiSTer device (facts)

- Host on the LAN: `192.168.1.117`, SSH `root` / password `1`.
- CPU/OS: ARM Cortex-A9 **armv7l**, minimal Linux, **glibc 2.31**. DE10-Nano,
  **1 GiB DDR3** (≈400 MB free in our tests).
- Stock Python is **3.9.6, stdlib only** — irrelevant now (Rust binary only).
- Framebuffer: `/dev/fb0`, driver **`MiSTer_fb`** (`/proc/fb` → `0 MiSTer_fb`),
  **1920×1080×32**, `rgba 8/16,8/8,8/0` → byte order is **B,G,R,X**
  (little-endian). No `/dev/dri` (no DRM/KMS).
- FB mode is set by writing `/sys/module/MiSTer_fb/parameters/mode` as
  `"<fmt> <rb> <width> <height> <stride>"`, e.g. `8888 1 1920 1080 7680`.
- No system fonts on MiSTer — the Rust build embeds DejaVu in the binary
  (`rust/ui/fonts/DejaVuSans.ttf`).
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
pyproject.toml          host tooling only (uv + paramiko for mister_ssh.py)
scripts/
  deploy-rust.sh        build + deploy mister-magic-fb to /media/fat/mister-magic/
  mister_ssh.py         paramiko helper — run/reboot/reboot-wait/wait/put/get
  capture-fb.sh         grab /dev/fb0 → PNG (via mister_ssh + raw_to_png.py)
  raw_to_png.py         stdlib-only BGRX dump → PNG
  audit-mister.sh       device sanity check
reference/              READ-ONLY clones (gitignored) — see §6
build/                  gitignored framebuffer PNG dumps
history/                experiment notes (framebuffer-experiments.md, screenshots)
rust/                   native armv7 frontend — see §12
  ui/app.slint          demo UI
  ui/fonts/DejaVuSans.ttf
  src/main.rs           subcommands: read | fb | ui
  src/fpga.rs           SPI + fb_enable_direct
  src/fb.rs             /dev/fb0 mmap, vsync, dirty-row copy
  build-arm.sh          cross build wrapper
```

On the device the binary lives at `/media/fat/mister-magic/mister-magic-fb`.

---

## 5. Workflow & commands

Always go through `uv` on the host; always use **paramiko** (`mister_ssh.py`)
for device comms — see §8 for why `expect`/raw `ssh` was unreliable.

```bash
# Build + deploy the Rust binary (~820 KB)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh

# Or build only:
rust/build-arm.sh

# Run on device (pause menu so we own the SPI bus — see §3):
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run \
  'MP=$(pidof MiSTer); kill -STOP $MP; /media/fat/mister-magic/mister-magic-fb ui 20; kill -CONT $MP'

# Device comms
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run "uname -a"
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py reboot
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py reboot-wait
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py wait

# Capture framebuffer → PNG
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh build/fb.png
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
| `reference/mister-companion` | `Anime0t4ku/mister-companion` | Desktop companion app; the **reliable MiSTer SSH approach** we modelled `mister_ssh.py` on (paramiko, `allow_agent=False`, `look_for_keys=False`). See §8. |

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
   `exec`s our `/media/fat/mister-magic/run-mister.sh`.
3. Re-enable `main=zaparoo/MiSTer_Zaparoo` in `MiSTer.ini` (uncomment line 278).
4. Reboot. `MiSTer_Zaparoo` loads `menu_zaparoo.rbf`, enables the fb, switches
   to tty2, and runs our launcher → Slint should appear on HDMI.
5. **Revert:** restore `zaparoo/frontend`, or re-comment `main=`, then reboot.

Caveats to verify by capture: exact fb dims Zaparoo sets for the HDMI (non-CRT)
path; whether the default CRT-persisted state forces `--crt` (our launcher
should ignore it); that Slint's linuxfb backend is happy on tty2 (it should be —
a real VT also fixes the `VT_GETSTATE ioctl failed` warning we saw over SSH).

### Option B — Reproduce `video_fb_enable(1,0)` ourselves (medium, clean-ish) — ✅ MECHANISM PROVEN

Write a tiny ARM helper that issues the same FPGA SPI sequence (port the
`spi_uio_cmd_cont(UIO_SET_FBUF)` + `spi_w(...)` calls from `fpga_io.cpp` /
`video.cpp`), plus `video_chvt`, then launch Slint. Frees us from Zaparoo but
needs the MiSTer ARM toolchain and `/dev/mem` FPGA access, and the *running
core must support the HPS framebuffer* (the stock menu core does).

**We validated this from our own process (see §9.5 FPGA spike).** A throwaway
Python port of the SPI layer, run on-device with the stock menu SIGSTOPped, DID
route `/dev/fb0` to HDMI — our test pattern appeared (proof the fork is not
required). It rendered as misaligned colour columns because of geometry, not
comms: the device runs `direct_video=1`, so `video_fb_enable` takes the
`xoff = v_cur.item[4] - FB_DV_LBRD`, `yoff = v_cur.item[8] - FB_DV_UBRD` path
(plus `set_vga_fb()`), which our `xoff=yoff=0` probe didn't replicate. The Rust
port must reuse MiSTer's full positioning math (and read the live mode), not
hardcode 1920×1080. The remaining "hard part" is the VT/graphics-mode context,
not the SPI.

### Option C — Ship our own `main=` frontend binary (heaviest)

Fork `Main_MiSTer` minimally so `main=mister-magic/...` boots a stub that sets
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
- **busybox has no `pkill`.** Use `kill -9 $(pidof mister-magic-fb)` to stop the app.
- **libinput quirks DB missing** → `libinput error: ... device quirks` warnings.
  Rendering is fine; if/when we add input, bundle the quirks DB or point
  libinput at one.
- **Fonts:** MiSTer has none. The Rust build embeds DejaVu (`rust/ui/fonts/`).
- **Framebuffer byte order is BGRX** (`rgba 8/16,8/8,8/0`). `raw_to_png.py`
  swaps B/R; keep that in mind for any direct fb work.
- **Don't leave the menu paused.** If you `kill -STOP $(pidof MiSTer)` for an
  experiment, always `kill -CONT` it afterwards (or reboot).
- **Slow SSH after reboot = DHCP, not sshd.** `sshd` listens ~kernel 9 s, but
  `eth0` is managed by `dhcpcd` (it's *not* in `/etc/network/interfaces`), and the
  default path wasted ~17 s: DHCP solicit timeout → IPv4LL (169.254.x) fallback →
  ARP DAD probing before the real lease. A static IP in `/etc/dhcpcd.conf`
  (`noarp`, `noipv4ll`) skips all of it. Rootfs is read-only — remount rw to edit.
  See §10.
- **FPGA SPI from our own process needs the bus to ourselves.** The stock MiSTer
  (or Zaparoo) process drives GPO/GPI continuously. To inject SPI from a separate
  process, `kill -STOP $(pidof MiSTer)` first, then `kill -CONT` after. HDMI stays
  alive while stopped (scan-out is FPGA-driven). GPO is write-only (reads return
  GPI), so you can't recover its shadow — start from `0x80000000` (BIT31) and only
  touch SPI bits. See §9.5.
- **Don't blind-sleep on reboot.** The device reboots fast (~35s to userspace,
  drops off the network in ~3s). Use `mister_ssh.py reboot-wait` (or `wait`),
  which detects the down→up transition and returns the instant `pidof MiSTer`
  answers, instead of a fixed `sleep`. Polling port 22 alone is not enough —
  it confirm-runs a command so we don't act before the rootfs is ready.

---

## 9. Performance (rendering)

See `history/2026-5-2/framebuffer-experiments.md` for the full experiment log.
The Python/Zaparoo path (~62fps, tearing, ~96% CPU) is superseded by the Rust
frontend at locked 60fps (§9.7).

### 9.5 FPGA direct-access spike — fork-free `/dev/fb0` → HDMI (✅ proven)

Goal: prove we can route `/dev/fb0` to HDMI **from our own process** (no Zaparoo),
as the foundation for a Rust `main=` frontend (Option C) and tear-free page-flip.
We validated the FPGA layer with a throwaway **Python** port of MiSTer's SPI code
(`history/2026-5-2/framebuffer-experiments.md` — the Python `scripts/fpga_*`
probes were removed; the real impl is Rust). Method: boot to the
stock menu, `kill -STOP $(pidof MiSTer)` to own the SPI bus (HDMI stays live —
scan-out is FPGA-driven), poke registers, `kill -CONT` after.

**Proven facts (these de-risk the Rust port):**

- **FPGA registers = `mmap(/dev/mem)` + volatile u32.** The HPS↔FPGA "SPI" is the
  FPGA-manager GPO/GPI pair at `0xFF706000 + 0x10` (out) / `+0x14` (in). Map that
  page, done. Addresses confirmed live.
- **The bit-bang handshake works from our process.** `fpga_spi(word)`: set data +
  strobe (`GPO bit17`), wait `GPI bit17` (ACK) high, drop strobe, wait ACK low.
  Instrumented: idle `GPI=0x00040000` (io_ver=1); strobe-high → `0x00060001`
  (ACK + data present); strobe-low → `0x00040000`. Real handshake, not a stale read.
- **Chip-select framing matters.** If MiSTer is SIGSTOPped mid-transaction, `IO_EN`
  (`GPO bit20`) may be high; drop then raise it (`DisableIO`→`EnableIO`) so the
  command word isn't mis-parsed as a parameter.
- **`video_fb_enable(1,0)` routes `/dev/fb0` to HDMI — confirmed visually.** Our
  test pattern replaced the wallpaper. So the **Zaparoo fork is not required** to
  own the screen.
- **`/dev/fb0` phys addr = `0x22001000`** (`smem_start`) = `FB_ADDR(0x22000000)
  + 4096` (the `n?0:4096` params page). Matches `video.cpp`.
- The 10-word SET_FBUF sequence (after `spi_w(UIO_SET_FBUF=0x2F)`): `fmt`
  (`FB_EN|FB_FMT_RxB|FB_FMT_8888 = 0x8016`), addr-lo, addr-hi, width, height,
  scaled L, scaled R, scaled T, scaled B, stride.

**Open issues for the Rust port (why the spike showed colour columns):**

- **`direct_video=1` on this device.** `video_fb_enable` then positions the fb via
  `xoff = v_cur.item[4] - FB_DV_LBRD(3)`, `yoff = v_cur.item[8] - FB_DV_UBRD(2)`,
  uses tiny border porches (`FB_DV_*`), and calls `set_vga_fb()`. Our probe used
  `xoff=yoff=0` (non-direct path) → misaligned columns. The Rust port must reuse
  MiSTer's positioning math (and the live mode `video_mode=8` = 1080p), not
  hardcode 1920×1080.
- **Multi-word SPI *reads* were flaky in Python.** Command-word reads work (got the
  `GET_FB_PAR` CRC `0x6d`, the SET_FBUF "supported" flag `0x1`), but `UIO_GET_VRES`
  / `UIO_GET_FB_PAR` data words all read 0 — a back-to-back read-timing artifact of
  slow Python (or the menu core not implementing them). At native Rust speed, port
  `fpga_spi` faithfully; MiSTer reads these fine.
- VT/graphics-mode context (so fbcon doesn't clobber, §8/§11) is still required —
  that's the genuinely fiddly part, not the SPI.

### 9.6 Native Rust port — clean fork-free image (✅ done)

The spike is now reproduced in Rust (`rust/` crate, §12) and renders a **clean,
full-screen image from our own binary with zero Zaparoo** — the de-forking
premise is proven end-to-end:

- `rust/src/fpga.rs` ports the SPI layer (`mmap` GPO/GPI, EnableIO/DisableIO,
  `fpga_spi`) and `video_fb_enable(1,n)` (the SET_FBUF sequence). **Native-speed
  multi-word reads work** (GET_VRES/GET_FB_PAR return stable data; ACK-high ==
  ACK-low), unlike the slow Python which read 0s.
- `rust/src/fb.rs` mmaps `/dev/fb0` (1920x1080 xRGB8888) for direct pixel writes.
- `mister-magic-fb fb [xoff] [yoff]` paints a 4-quadrant + border + cross-hair
  test pattern and routes buffer 0. `read` dumps the live mode/fb params.
- Live values read from the stock menu: `GET_FB_PAR` → `fb_w=1920 fb_h=1080
  fb_fmt=0x00d6 fb_en=1`; `GET_VRES` → `width=529 height=240 pixrep=2` (the
  direct_video native scan-out, not the fb size).

**Geometry lesson (why the columns, then the offset):**

- Columns = wrong horizontal *span*. The scaled right coord must be
  `xoff + v_cur.item[1] - 1`; with `item[1]=1920` that's a 1920-wide span in the
  direct_video raster (which is ~2147 wide). Sending `right=1919` from `left=0`
  is still a 1920 span, but earlier Python also mis-sent params; the Rust port
  with the correct sequence does **not** shear.
- Offset = wrong `xoff/yoff`. The `direct_video` formula is
  `xoff=item[4]-FB_DV_LBRD(3)`, `yoff=item[8]-FB_DV_UBRD(2)`. Using the *original*
  mode-8 porches (`148/36`) put us ~145px too far right. The **running menu mode**
  already has the tiny border porches (`item[4]=3, item[8]=2`), so the correct
  values are **`xoff=yoff=0`**. Confirmed clean on HDMI at `0,0`.

**Resolution / CRT generality (answer to "will this cope with lower res / CRT?"):**
Yes, but not by hardcoding. `MODE_1080P60` and the `0,0` offset are specific to
this 1080p direct_video menu. The robust path (todo `rust-livemode`) is to read
the **live** mode each time — `UIO_GET_VRES` for the active resolution, plus the
mode timing for the porches — and compute `fb_width/height`, `xoff/yoff`, and the
scaled coords from that. Our own `main=` frontend will set/own its mode, so it
always knows the correct geometry (including low-res and CRT/`direct_video`
outputs). The plumbing already reads these registers; only the derivation is TODO.

### 9.7 Slint software renderer @ locked 60fps — smooth + tear-free (✅ done)

`mister-magic-fb ui [secs]` runs Slint's **software renderer** directly on the
framebuffer (no X/Wayland, no Zaparoo) at a **rock-steady 60fps, smooth and
tear-free** (confirmed on HDMI). Per-frame budget (1080p, animated demo UI):

```
render 2.3ms (cached RAM)  +  vsync-wait ~5.6ms  +  dirty-row copy ~8.7ms  ≈ 16.6ms
```

**Architecture (the bits that matter):**

- `MisterPlatform` implements Slint's `Platform` trait: one `MinimalSoftwareWindow`
  (`RepaintBufferType::ReusedBuffer`), time from a monotonic `Instant`. We drive
  the loop ourselves (no `run_event_loop`), pacing each frame on
  `FBIO_WAITFORVSYNC`.
- Render into a **cached** `Vec<Pixel>` (fast, ~2.3ms — Slint only redraws the
  dirty region). `render()` returns a `PhysicalRegion`; we take its bounding-box
  rows.
- After `wait_vsync`, copy **only the dirty rows** into `/dev/fb0`. Single
  buffer, routed once via `fb_enable_direct(0, …, 0, 0)`.

**THE key hardware finding — write-combining vs uncached:**

- `/dev/fb0`'s driver mmap is **write-combining (~700 MB/s)**.
- mmapping the *same* physical buffers via `/dev/mem` (as MiSTer's `shmem_map`
  does, `O_RDWR|O_SYNC`) is **uncached device memory (~105 MB/s)** — ~7× slower.
- Consequence: **true FPGA page-flipping is a dead end here.** It's genuinely
  tear-free and the flip itself is ~12µs, BUT the back buffers (1/2) are only
  reachable via `/dev/mem`, so writing them is too slow — a 620-row update took
  **45ms** (→ 20fps). Rendering *directly* into a `/dev/mem` buffer
  (`SwappedBuffers`) was ~15–17ms and unstable (dropped to 30fps under load).
  `/dev/fb0` only ever exposes **one** buffer (`virtual_size` = 1920×1080), so we
  can't get a second *write-combining* buffer to flip between.
- The winning compromise: single write-combined buffer + dirty-row copy started
  right after vblank. The copy (~8.7ms for 619 rows) stays just ahead of the scan
  beam (which needs ~9.5ms for those rows), so it reads as tear-free in practice.

**Slint build notes (cross-compile, no system fonts):**

- Build Slint with `default-features=false`, features
  `["compat-1-2","renderer-software","unsafe-single-threaded","libm"]` — **no
  `std`** (the `std` feature pulls system font loading → `fontconfig`, which the
  bare cross container and the MiSTer don't have). Our own crate still uses `std`.
- `build.rs` uses `EmbedResourcesKind::EmbedForSoftwareRenderer` so glyphs/images
  are baked into the binary. With no system fonts, the embedder needs a font:
  we bundle `rust/ui/fonts/DejaVuSans.ttf` (Bitstream Vera license) and set
  `default-font-family: "DejaVu Sans"` on the Window.
- Slint's deps need **rustc ≥ 1.90**; the toolchain is pinned to `stable`
  (1.96 at time of writing) in `rust/rust-toolchain.toml`, with a matching
  `stable-x86_64-unknown-linux-gnu` installed (`--force-non-host`) for the
  emulated cross container.
- `profile.release` uses `opt-level = 3` (not `"z"`): the software renderer is
  hot. Binary is ~820KB incl. embedded font.

**Known follow-ups for the real frontend:** the copy is still ~half the screen
because the demo's gradient bar spans full width (full-width dirty rows). A real
UI with localised motion will copy far less. Could also copy only the dirty
*sub-rectangle* (x-range) instead of full-width rows for further savings.

---

## 10. Current device state & recovery

- **Currently at the STOCK menu** (Zaparoo disabled): `MiSTer.ini` line 278 is
  `;main=zaparoo/MiSTer_Zaparoo` (commented). Backup: `/media/fat/MiSTer.ini.bak`.
- `direct_video=1`, `video_mode=8` (1080p) in `MiSTer.ini` — relevant to fb
  positioning (see §9.5).
- **Static IP `192.168.1.117` (no DHCP).** See §8 for dhcpcd.conf details.
- Rust binary deployed at `/media/fat/mister-magic/mister-magic-fb` (~820 KB).
- **Black-screen recovery:** paramiko SSH works even with no usable video. To
  recover from a bad `main=` experiment: SSH in, re-comment `main=` (or restore
  `MiSTer.ini.bak`), then `mister_ssh.py reboot`.

---

## 11. Open questions / follow-ups

- **Cleaner fix than the always-on animation** in the demo UI (full-width dirty
  rows). A real UI with localised motion copies less. fbcon may still clobber a
  static frame — investigate `KD_GRAPHICS` / unbind fbcon (see §8).
- Input: map controller/keyboard to Slint. Zaparoo maps `JOY_L2/R2/OSD` →
  `F1/Backspace/Menu` (`alt_launcher.cpp:49-66`); we'll need our own scheme and
  an "exit" key (our app currently runs forever with no quit path on-device).
- libinput quirks DB is missing — fine for rendering; revisit when wiring input.

---

## 12. Rust ARM toolchain (for the native frontend / Option C)

We're building a native armv7 frontend in Rust (`rust/` crate, see §9.5 for why:
fork-free fb routing + tear-free vsync + Slint's Rust software renderer). The
cross-compile toolchain is **proven end-to-end**: a binary built on the
Apple-Silicon host runs on the MiSTer (`arch=arm, os=linux, glibc OK`).

**Build & deploy:**

```bash
scripts/deploy-rust.sh                   # build + deploy in one step
# or manually:
rust/build-arm.sh
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py \
  put rust/target/armv7-unknown-linux-gnueabihf/release/mister-magic-fb /media/fat/mister-magic/mister-magic-fb
```

**One-time host setup (done):**

```bash
cargo install cross --locked
rustup toolchain add stable-x86_64-unknown-linux-gnu --profile minimal --force-non-host
```

**Apple-Silicon gotchas (all handled by `build-arm.sh` + config, but know them):**

- **glibc match:** `cross` 0.2.5 images are Ubuntu 20.04 = **glibc 2.31**, which is
  what the MiSTer runs. So the default image Just Works — no musl, no static.
- **`--force-non-host`:** cross mounts the host's rustup toolchain into the Linux
  container, so the `*-unknown-linux-gnu` toolchain must be installed on the Mac
  even though it can't run there. rustup blocks it without `--force-non-host`.
- **`DOCKER_DEFAULT_PLATFORM=linux/amd64`:** the cross image has no arm64 manifest;
  on arm64 Docker we must request amd64 (qemu-emulated). Our crate is tiny so the
  emulation cost is negligible. (`build-arm.sh` sets this.)
- **sccache wrapper:** the global `~/.cargo/config.toml` sets
  `rustc-wrapper=/opt/homebrew/bin/sccache` — a macOS path that doesn't exist in
  the container. `rust/.cargo/config.toml` overrides it to empty.
- **toolchain pin:** `rust/rust-toolchain.toml` pins stable `1.88` + the armv7
  target (the host default is nightly, which tripped cross's provisioning).

**Crate layout:**
- `rust/src/fpga.rs` — SPI layer + `video_fb_enable` port (§9.6), UIO/FB constants.
- `rust/src/fb.rs` — `/dev/fb0` mmap wrapper for direct pixel writes.
- `rust/src/main.rs` — `read` (dump live mode) and `fb` (test pattern + route)
  subcommands. Run with the stock-menu SIGSTOP dance (§9.5).

Next: derive geometry from the live mode (`rust-livemode`), wire input, ship as
a `main=` boot binary.
