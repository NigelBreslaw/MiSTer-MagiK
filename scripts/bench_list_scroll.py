#!/usr/bin/env python3
"""Run list_scroll bench on MiSTer at render_scale 1 and 2. Non-interactive."""
from __future__ import annotations

import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

# Reuse mister_ssh connect (paramiko, no agent keys, no prompts).
sys.path.insert(0, str(Path(__file__).resolve().parent))
from mister_ssh import connect, run  # noqa: E402

HERE = Path(__file__).resolve().parent.parent
REMOTE = "/media/fat/mister-magic/mister-magic-fb"
TSV = HERE / "history/toolchain-bench/results.tsv"
BENCH_DIR = HERE / "history/toolchain-bench"
SECS = int(os.environ.get("BENCH_SECS", "15"))
SCALES = os.environ.get("BENCH_SCALES", "1,2").split(",")

FPS_RE = re.compile(
    r"fps ~ (\d+).*render (\d+)us.*vsync-wait (\d+)us.*copy (\d+)us.*\((\d+) (?:logical )?rows avg\)"
)


def parse_log(text: str) -> tuple[int, int, int, int, int] | None:
    rows = []
    for line in text.splitlines():
        m = FPS_RE.search(line)
        if m:
            rows.append(tuple(int(m.group(i)) for i in range(1, 6)))
    rows = rows[3:]
    if not rows:
        return None
    n = len(rows)
    return (
        sum(r[1] for r in rows) // n,
        sum(r[2] for r in rows) // n,
        sum(r[3] for r in rows) // n,
        sum(r[4] for r in rows) // n,
        sum(r[0] for r in rows) // n,
    )


def bench_scale(client, label: str, scale: int, bin_bytes: int) -> None:
    capture_at = SECS - 2 if SECS > 4 else 2
    cmd = f"""
MP=$(pidof MiSTer 2>/dev/null || true)
[ -n "$MP" ] && kill -STOP $MP
MISTER_RENDER_SCALE={scale} {REMOTE} ui list_scroll {SECS} > /tmp/bench-ui.log 2>&1 &
UI_PID=$!
CPU_SUM=0; CPU_N=0
TICK=$(getconf CLK_TCK 2>/dev/null || echo 100)
jiffies() {{ awk '{{print $14+$15}}' /proc/$1/stat 2>/dev/null || echo 0; }}
FB=0
i=0
while [ $i -lt {SECS} ]; do
  if kill -0 $UI_PID 2>/dev/null; then
    if [ $FB -eq 0 ] && [ $i -ge {capture_at} ]; then
      dd if=/dev/fb0 of=/tmp/bench-fb.raw bs=1M count=8 2>/dev/null && FB=1
    fi
    t1=$(jiffies $UI_PID); sleep 1; t2=$(jiffies $UI_PID)
    p=$(( (t2 - t1) * 100 / TICK )); [ "$p" -lt 0 ] 2>/dev/null && p=0
    CPU_SUM=$((CPU_SUM + p)); CPU_N=$((CPU_N + 1))
  else
    sleep 1
  fi
  i=$((i + 1))
done
wait $UI_PID
[ -n "$MP" ] && kill -CONT $MP
echo ___CPU___ $((CPU_N > 0 ? CPU_SUM / CPU_N : 0))
echo ___LOG___
cat /tmp/bench-ui.log
"""
    stdin, stdout, stderr = client.exec_command(cmd, timeout=SECS + 60, get_pty=False)
    stdin.close()
    out = stdout.read().decode("utf-8", "ignore")
    stdout.channel.recv_exit_status()

    log = out.split("___LOG___", 1)[-1].lstrip("\n")
    cpu_m = re.search(r"___CPU___ (\d+)", out)
    cpu_mean = cpu_m.group(1) if cpu_m else ""

    ui_log = BENCH_DIR / f"{label}-list_scroll-ui.log"
    ui_log.write_text(log)

    stats = parse_log(log)
    visual_ok = "yes" if stats and "done:" in log else "no"
    notes = f"render_scale={scale}; design=960x540"
    notes += "; render=1920x1080" if scale == 2 else "; render=960x540; fb_scale=2"

    png = BENCH_DIR / f"{label}-list_scroll-fb.png"
    try:
        sftp = client.open_sftp()
        sftp.get("/tmp/bench-fb.raw", str(HERE / "build/bench-fb.raw"))
        sftp.close()
        os.system(
            f"python3 {HERE / 'scripts/raw_to_png.py'} "
            f"{HERE / 'build/bench-fb.raw'} 1920 1080 {png} >/dev/null 2>&1"
        )
    except OSError:
        visual_ok = "no"

    if stats:
        render_us, vsync_us, copy_us, rows_avg, fps_val = stats
        print(
            f"=== {label} render_scale={scale} ===\n"
            f"  render={render_us}us vsync={vsync_us}us copy={copy_us}us "
            f"rows={rows_avg} fps={fps_val} cpu={cpu_mean}%"
        )
    else:
        render_us = vsync_us = copy_us = rows_avg = fps_val = ""
        print(f"=== {label} render_scale={scale} === no fps lines", file=sys.stderr)

    row = (
        f"{label}\tlist_scroll\t{datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}\t"
        f"1.96.0\t\t{bin_bytes}\t{render_us}\t{vsync_us}\t{copy_us}\t{rows_avg}\t{fps_val}\t"
        f"{cpu_mean}\t\t\t{visual_ok}\t{notes}\n"
    )
    with TSV.open("a") as f:
        f.write(row)


def main() -> int:
    bin_path = HERE / "rust/target/armv7-unknown-linux-gnueabihf/release-device/mister-magic-fb"
    if not bin_path.is_file():
        print(f"missing binary: {bin_path}", file=sys.stderr)
        return 1
    bin_bytes = bin_path.stat().st_size

    client = connect()
    try:
        run(client, "kill -CONT $(pidof MiSTer) 2>/dev/null; true")
        sftp = client.open_sftp()
        sftp.put(str(bin_path), f"{REMOTE}.new")
        sftp.close()
        run(client, f"mv {REMOTE}.new {REMOTE} && chmod +x {REMOTE}")

        for scale_s in SCALES:
            scale = int(scale_s.strip())
            label = f"LS-rs{scale}"
            bench_scale(client, label, scale, bin_bytes)
    finally:
        client.close()

    print(f"Appended to {TSV}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
