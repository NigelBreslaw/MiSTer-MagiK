#!/usr/bin/env python3
"""Sweep dirty_band band-pct on MiSTer; find max % that still hits ~60 fps.

Usage:
  MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench_dirty_band.sh
  BENCH_PCTS=25,50,75,100 BENCH_SECS=10 scripts/bench_dirty_band.sh

Builds release-device if binary missing/stale; deploys; runs each pct with kill prep.
"""
from __future__ import annotations

import os
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from mister_ssh import connect, run  # noqa: E402

HERE = Path(__file__).resolve().parent.parent
REMOTE = "/media/fat/mister-magic/mister-magic-fb"
BIN = HERE / "magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magic-fb"
OUT_DIR = HERE / "history/toolchain-bench"
TSV = OUT_DIR / "results.tsv"

SECS = int(os.environ.get("BENCH_SECS", "12"))
PCTS = [int(x.strip()) for x in os.environ.get("BENCH_PCTS", "10,20,30,40,50,60,70,80,90,100").split(",")]
FPS_TARGET = float(os.environ.get("BENCH_FPS_TARGET", "59.0"))
LABEL = os.environ.get("BENCH_LABEL", "dirty-band")

FPS_LINE = re.compile(
    r"fps ~ (\d+).*render (\d+)us.*vsync-wait (\d+)us.*copy (\d+)us.*\((\d+) (?:logical )?rows avg\)"
)
DONE_LINE = re.compile(r"done: (\d+) frames in ([\d.]+)s = ([\d.]+) fps avg")


@dataclass
class Sample:
    pct: int
    fps: float
    render_us: int
    vsync_us: int
    copy_us: int
    rows: int


def ensure_binary() -> int:
    slint = HERE / "magik-gui/ui/bench/dirty_band.slint"
    if not BIN.is_file() or slint.stat().st_mtime > BIN.stat().st_mtime:
        print("==> building release-device (dirty_band changed or binary missing)")
        subprocess.run(
            [str(HERE / "magik-gui/build-arm.sh"), "--device"],
            cwd=HERE / "magik-gui",
            check=True,
        )
    return BIN.stat().st_size


def parse_log(text: str) -> tuple[float, int, int, int, int] | None:
    fps_lines = []
    for line in text.splitlines():
        m = FPS_LINE.search(line)
        if m:
            fps_lines.append(tuple(int(m.group(i)) for i in range(1, 6)))
    done_fps = None
    for line in text.splitlines():
        m = DONE_LINE.search(line)
        if m:
            done_fps = float(m.group(3))

    if len(fps_lines) <= 3:
        if done_fps is not None:
            return done_fps, 0, 0, 0, 0
        return None

    fps_lines = fps_lines[3:]
    n = len(fps_lines)
    avg = (
        sum(r[0] for r in fps_lines) / n,
        sum(r[1] for r in fps_lines) // n,
        sum(r[2] for r in fps_lines) // n,
        sum(r[3] for r in fps_lines) // n,
        sum(r[4] for r in fps_lines) // n,
    )
    fps = done_fps if done_fps is not None else avg[0]
    return fps, avg[1], avg[2], avg[3], avg[4]


def bench_pct(client, pct: int, bin_bytes: int) -> Sample | None:
    cmd = f"""
kill -9 $(pidof mister-magic-fb) 2>/dev/null || true
kill -9 $(pidof MiSTer) 2>/dev/null || true
sleep 0.5
MISTER_DIRTY_BAND_PCT={pct} {REMOTE} ui dirty_band {SECS} > /tmp/bench-dirty-band.log 2>&1
cat /tmp/bench-dirty-band.log
"""
    stdin, stdout, stderr = client.exec_command(cmd, timeout=SECS + 90, get_pty=False)
    stdin.close()
    out = stdout.read().decode("utf-8", "ignore")
    stdout.channel.recv_exit_status()

    log_path = OUT_DIR / f"{LABEL}-dirty_band-pct{pct}-ui.log"
    log_path.write_text(out)

    parsed = parse_log(out)
    if not parsed:
        print(f"  pct={pct:3d}%  FAILED (no metrics)", file=sys.stderr)
        return None

    fps, render_us, vsync_us, copy_us, rows = parsed
    print(
        f"  pct={pct:3d}%  fps={fps:5.1f}  render={render_us:5d}us  "
        f"copy={copy_us:5d}us  rows={rows:3d}  ({rows * 100 // 540}% height)"
    )

    row = (
        f"{LABEL}-p{pct}\tdirty_band\t{datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}\t"
        f"1.96.0\t\t{bin_bytes}\t{render_us}\t{vsync_us}\t{copy_us}\t{rows}\t{int(round(fps))}\t"
        f"\t\t\t{'yes' if 'done:' in out else 'no'}\t"
        f"band_pct={pct}; prep=kill-mister-ui; profile=release-device\n"
    )
    with TSV.open("a") as f:
        f.write(row)

    return Sample(pct=pct, fps=fps, render_us=render_us, vsync_us=vsync_us, copy_us=copy_us, rows=rows)


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    bin_bytes = ensure_binary()

    client = connect()
    try:
        run(client, "kill -CONT $(pidof MiSTer) 2>/dev/null; true")
        sftp = client.open_sftp()
        sftp.put(str(BIN), f"{REMOTE}.new")
        sftp.close()
        run(client, f"mv {REMOTE}.new {REMOTE} && chmod +x {REMOTE}")

        print(f"==> dirty_band sweep  pcts={PCTS}  secs={SECS}  target>={FPS_TARGET} fps")
        print(f"    {'pct':>5}  {'fps':>6}  {'render':>8}  {'copy':>8}  {'rows':>5}  notes")
        results: list[Sample] = []
        for pct in PCTS:
            s = bench_pct(client, pct, bin_bytes)
            if s:
                results.append(s)
    finally:
        client.close()

    if not results:
        print("No results.", file=sys.stderr)
        return 1

    ok = [s for s in results if s.fps >= FPS_TARGET]
    max_ok = max(ok, key=lambda s: s.pct) if ok else None
    first_fail = next((s for s in results if s.fps < FPS_TARGET), None)

    print()
    if max_ok:
        print(
            f"60 fps ceiling (solid band): ~{max_ok.pct}%  "
            f"(fps={max_ok.fps:.1f}, rows≈{max_ok.rows}, copy≈{max_ok.copy_us}us)"
        )
    else:
        print(f"No pct reached {FPS_TARGET} fps in this sweep.")

    if first_fail and (not max_ok or first_fail.pct > max_ok.pct):
        print(
            f"First drop below target: {first_fail.pct}%  "
            f"(fps={first_fail.fps:.1f}, render={first_fail.render_us}us, copy={first_fail.copy_us}us)"
        )

    print(f"Logs + TSV rows under {OUT_DIR} (label prefix {LABEL})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
