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

- ✅ **Native Rust frontend** — `mister-magic-fb ui` at locked 60fps, smooth +
  tear-free. See §9.7.
- ✅ FPGA SPI + `video_fb_enable(1,0)` in `rust/src/fpga.rs`.
- ✅ Custom Slint `Platform` + dirty-row copy in `rust/src/fb.rs` / `main.rs`.

See `history/2026-5-2/framebuffer-experiments.md` for the exploration path.

Remaining work: input, live-mode geometry, shipping as a `main=` boot binary (§7).

---

## 2. The MiSTer device (facts)

- Host on the LAN: `192.168.1.117`, SSH `root` / password `1`.
- CPU/OS: ARM Cortex-A9 **armv7l**, minimal Linux, **glibc 2.31**. DE10-Nano,
  **1 GiB DDR3** (≈400 MB free in our tests).
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

So the wallpaper you see is the menu's wallpaper **buffer
`menu_bgn`**, drawn by the main binary (`draw_*`/`video_menu_bg` in `video.cpp`).
**Buffer 0 (`/dev/fb0`) is never scanned at the menu.** Our Slint app writes
buffer 0 correctly (the PNG dumps prove it) but the FPGA isn't looking at it.

**Empirical confirmation:** we paused the menu process (`kill -STOP $(pidof
MiSTer)`) and the Slint frame *still* didn't appear on HDMI — ruling out simple
"menu paints over us" contention. The framebuffer dump showed Slint; the screen
did not. Only `video_fb_enable(1, 0)` (FPGA SPI, main-binary only) makes buffer
0 visible.

### 3.2 The `main=` boot hook is stock MiSTer

`main=` is a **native** MiSTer.ini option:

- `reference/Main_MiSTer/cfg.cpp:141` maps INI key `MAIN` → `cfg.main`.
- `cfg.cpp:612` default is `cfg.main = "MiSTer"`.
- `reference/Main_MiSTer/user_io.cpp:1463` re-execs the configured binary.
- Stock `reference/Main_MiSTer/MiSTer.ini:381` documents `;main=some_binary_file`.

Setting `main=some/path/to/binary` makes `/media/fat/MiSTer` re-exec that binary
at boot instead of the stock menu. **Future production path** for this project
(§7); we don't use it yet — dev runs use the SIGSTOP dance (§5).

### 3.3 Engineering takeaway

To show Slint on HDMI we need:

1. **`video_fb_enable(1, 0)`** — FPGA SPI (`SET_FBUF`) pointing scan-out at buffer 0.
2. **Correct geometry** — `xoff`/`yoff` and scaled coords for `direct_video=1`
   (see §9.6; currently hardcoded `0,0` for this 1080p menu).
3. **A render loop** — custom Slint `Platform`, vsync pacing, dirty-row copy into
   write-combined `/dev/fb0` (§9.7).

We issue the SPI sequence ourselves from Rust (`rust/src/fpga.rs`). The stock
`MiSTer` process normally owns the SPI bus; for dev we `kill -STOP $(pidof MiSTer)`
first (§8). A `main=` boot binary would own the bus from the start — no pause
dance, plus proper VT/fbcon setup (still TODO).

**Zaparoo is not used and not required.** It was an early launch path (forked
`Main_MiSTer` that calls `video_fb_enable` + spawns a frontend). We proved the
fork is unnecessary by issuing the same SPI from our own process (§9.5).

---

## 4. Repo layout (this project)

```
pyproject.toml          host tooling only (uv + paramiko for mister_ssh.py)
scripts/
  deploy-rust.sh        build + deploy mister-magic-fb to /media/fat/mister-magic/
  mister_ssh.py         paramiko helper — run/reboot/reboot-wait/wait/put/get
  capture-fb.sh         grab /dev/fb0 → PNG (via mister_ssh + raw_to_png.py)
  raw_to_png.py         stdlib-only BGRX dump → PNG
  audit-mister.sh       device sanity check (+ Cortex-A9 / NEON cpuinfo for A1)
reference/              READ-ONLY clones (gitignored) — see §6
build/                  gitignored framebuffer PNG dumps
history/                experiment notes (framebuffer-experiments.md, screenshots)
rust/                   native armv7 frontend — see §12
  ui/app.slint          demo UI
  ui/bench/*.slint      toolchain/visual bench scenes (incl. list_scroll)
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

# Capture framebuffer → PNG (only valid while `ui` is still running)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh build/fb.png

# Toolchain A/B (host build + all ui/bench scenes on device) → history/toolchain-bench/results.tsv
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh A0 --clean --replace-label
```

Bench scenes: `mister-magic-fb scenes` or `ui <scene> 20` — see `rust/ui/bench/README.md`.
Log: [`history/toolchain-bench/README.md`](history/toolchain-bench/README.md).

**Debug trick — see Slint without HDMI routing:** dump `/dev/fb0` and convert to
PNG **while `mister-magic-fb ui` is running**. After exit, fbcon shows `login:` and
the dump is useless. `bench-toolchain.sh` snapshots at ~`scene_secs - 2` s mid-run.

---

## 6. Reference source (cloned, read-only, gitignored)

Cloned under `reference/` (shallow). For study only; do not edit/commit.

| Dir | Repo | Why it matters |
|-----|------|----------------|
| `reference/Main_MiSTer` | `ZaparooProject/Main_MiSTer` (fork) | **`video.cpp`** (`video_fb_enable` :3284), **`fpga_io.cpp`/`spi.cpp`** (SPI bit-bang), **`cfg.cpp`** (`main=` hook). Read these, not the Zaparoo patches. |
| `reference/mister-companion` | `Anime0t4ku/mister-companion` | Reliable MiSTer SSH — modelled `mister_ssh.py` on this. See §8. |

Optional clones (historical — explored other front ends, **not used by this project**):
`Menu_MiSTer`, `zaparoo-frontend`, `zaparoo-core`, `Zaparoo_MiSTer`.

Key files: `reference/Main_MiSTer/video.cpp` (around `video_fb_enable`),
`fpga_io.cpp`, `user_io.h` (UIO/FB constants).

To refresh: `git -C reference/<repo> pull` (or re-clone `--depth 1`).

---

## 7. Architecture & roadmap

### Current — dev binary (`mister-magic-fb`)

Cross-built Rust binary deployed to `/media/fat/mister-magic/mister-magic-fb`.
Subcommands: `read` (SPI diagnostics), `fb` (geometry test pattern), `ui`
(Slint demo, locked 60fps).

Dev workflow pauses the stock menu to own the SPI bus (§5). The binary calls
`fb_enable_direct(0, …)` then runs the Slint render loop (§9.7). This is proven
end-to-end on HDMI.

### Next — `main=` boot binary

Ship a minimal binary (fork `Main_MiSTer` or standalone stub) as
`main=mister-magic/...` in `MiSTer.ini` so it boots directly into our frontend:
owns SPI from the start, sets up VT/fbcon properly, no SIGSTOP dance. Most
control, most work, must track upstream MiSTer.

### TODO

- Derive `xoff/yoff`/geometry from the **live** video mode (`rust-livemode`).
- Controller/keyboard input.
- ~~fbcon / `KD_GRAPHICS`~~ — `vt.rs` on `ui`/`fb` (static UI may still need more if fbcon clears the buffer).

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
- **fbcon clobbers `/dev/fb0` → black screen.** The kernel framebuffer console
  (`vtcon1`) can clear the buffer. The demo UI uses `animation-tick()` for
  continuous repaints; a static UI may need `KD_GRAPHICS` / unbind fbcon (§11).
- **`ui` / `fb` call `KD_GRAPHICS` on `/dev/tty0`** (`rust/src/vt.rs`) so fbcon
  stops drawing the blinking block cursor over the title (confirmed in framebuffer
  PNGs). Restores `KD_TEXT` on exit. If the ioctl fails, we log and continue.
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
- **FPGA SPI from our own process needs the bus to ourselves.** The stock
  `MiSTer` process drives GPO/GPI continuously. To inject SPI from a separate
  process, `kill -STOP $(pidof MiSTer)` first, then `kill -CONT` after. HDMI
  stays alive while stopped (scan-out is FPGA-driven). GPO is write-only (reads
  return GPI), so you can't recover its shadow — start from `0x80000000` (BIT31)
  and only touch SPI bits. See §9.5. A `main=` boot binary avoids this.
- **Don't blind-sleep on reboot.** The device reboots fast (~35s to userspace,
  drops off the network in ~3s). Use `mister_ssh.py reboot-wait` (or `wait`),
  which detects the down→up transition and returns the instant `pidof MiSTer`
  answers, instead of a fixed `sleep`. Polling port 22 alone is not enough —
  it confirm-runs a command so we don't act before the rootfs is ready.

---

## 9. Performance (rendering)

See `history/2026-5-2/framebuffer-experiments.md` for the full experiment log.

### 9.5 FPGA direct-access spike — fork-free `/dev/fb0` → HDMI (✅ proven)

Goal: prove we can route `/dev/fb0` to HDMI **from our own process**, as the
foundation for the Rust frontend and (eventually) a `main=` boot binary.
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
- **`video_fb_enable(1,0)` routes `/dev/fb0` to HDMI — confirmed visually.**
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

The spike is now reproduced in Rust (`rust/` crate) and renders a **clean,
full-screen image from our own binary**:

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
framebuffer (no X/Wayland) at a **rock-steady 60fps, smooth and tear-free**
(confirmed on HDMI). Per-frame budget (1080p, animated demo UI):

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

- **Stock menu** — `MiSTer.ini` has `main=MiSTer` (default). Backup:
  `/media/fat/MiSTer.ini.bak`.
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
- Input: map controller/keyboard to Slint; design an exit key (app runs until
  killed today).
- libinput quirks DB is missing — fine for rendering; revisit when wiring input.

---

## 12. Rust ARM toolchain

Cross-compile toolchain is **proven end-to-end**: a binary built on the
Apple-Silicon host runs on the MiSTer (`arch=arm, os=linux, glibc 2.31`).

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
- **toolchain pin:** `rust/rust-toolchain.toml` pins `stable` + the armv7 target.

**Crate layout:**
- `rust/src/fpga.rs` — SPI layer + `video_fb_enable` port (§9.6), UIO/FB constants.
- `rust/src/fb.rs` — `/dev/fb0` mmap wrapper for direct pixel writes.
- `rust/src/main.rs` — `read` | `fb` | `ui` subcommands.

See §7 for roadmap.
