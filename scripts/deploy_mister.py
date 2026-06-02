#!/usr/bin/env python3
"""Deploy the bundle to a MiSTer with live feedback.

Shows:
  * upload percentage + throughput (paramiko SFTP progress callback),
  * extraction progress (files extracted / total) streamed live from tar,
  * MiSTer CPU load average + free RAM, sampled on a second connection.

It also prunes the bundle (test suites, __pycache__, pip, tkinter, ...) to cut
the number of small files written to the slow exFAT card.

    MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/deploy_mister.py
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import paramiko

HERE = Path(__file__).resolve().parent.parent
BUNDLE = HERE / "build" / "mister-slint"
TARBALL = HERE / "build" / "mister-slint.tar.gz"
ENTRY = HERE / "deploy" / "mister-slint.sh"
REMOTE_TAR = "/media/fat/mister-slint.tar.gz"
REMOTE_APP = "/media/fat/mister-slint"
REMOTE_ENTRY = "/media/fat/Scripts/mister-slint.sh"

IP = os.environ.get("MISTER_IP", "192.168.1.117")
USER = os.environ.get("MISTER_USER", "root")
PASS = os.environ.get("MISTER_PASS", "1")


def human(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.1f}{unit}"
        n /= 1024
    return f"{n:.1f}TB"


def connect(timeout: float = 15.0) -> paramiko.SSHClient:
    c = paramiko.SSHClient()
    c.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    c.connect(IP, username=USER, password=PASS, timeout=timeout,
              banner_timeout=timeout, auth_timeout=timeout,
              allow_agent=False, look_for_keys=False)
    return c


def prune_bundle() -> None:
    print("==> Pruning bundle to cut file count")
    lib = BUNDLE / "python" / "lib" / "python3.12"
    for sub in ("test", "idlelib", "turtledemo", "tkinter", "lib2to3", "ensurepip"):
        shutil.rmtree(lib / sub, ignore_errors=True)
    sp = lib / "site-packages"
    for pat in ("pip", "pip-*", "setuptools", "setuptools-*", "pkg_resources", "_distutils_hack"):
        for p in sp.glob(pat):
            shutil.rmtree(p, ignore_errors=True) if p.is_dir() else p.unlink(missing_ok=True)
    for pyc in BUNDLE.rglob("__pycache__"):
        shutil.rmtree(pyc, ignore_errors=True)


def count_entries() -> int:
    n = 0
    for _root, dirs, files in os.walk(BUNDLE):
        n += len(dirs) + len(files)
    return n + 1  # include the top dir


def pack() -> None:
    print("==> Packing tarball (dereferencing symlinks for exFAT)")
    TARBALL.unlink(missing_ok=True)
    subprocess.run(
        ["tar", "-czhf", str(TARBALL), "-C", str(BUNDLE.parent), BUNDLE.name],
        check=True,
    )
    print(f"    {TARBALL.name}: {human(TARBALL.stat().st_size)}")


def upload() -> None:
    size = TARBALL.stat().st_size
    print(f"==> Uploading {human(size)} to {IP}")
    c = connect()
    sftp = c.open_sftp()
    start = time.time()
    last = [0.0]

    def cb(done: int, total: int) -> None:
        now = time.time()
        if now - last[0] > 0.4 or done >= total:
            last[0] = now
            pct = done * 100 / total if total else 100
            rate = done / (now - start) if now > start else 0
            sys.stdout.write(f"\r    upload {human(done)}/{human(total)} ({pct:5.1f}%) {human(rate)}/s   ")
            sys.stdout.flush()

    sftp.put(str(TARBALL), REMOTE_TAR, callback=cb)
    sys.stdout.write("\n")
    sftp.close()
    c.close()


def start_load_sampler(stop: threading.Event) -> threading.Thread:
    def sample() -> None:
        try:
            c = connect()
        except Exception as e:  # noqa: BLE001
            print(f"    [mister] load sampler could not connect: {e}")
            return
        while not stop.is_set():
            try:
                _in, out, _err = c.exec_command(
                    "cut -d' ' -f1-3 /proc/loadavg; awk '/MemAvailable/{print $2}' /proc/meminfo",
                    timeout=10,
                )
                parts = out.read().decode("utf-8", "ignore").split()
                if len(parts) >= 4:
                    load = " ".join(parts[:3])
                    mem = int(parts[3]) // 1024
                    print(f"    [mister] load {load} | memAvail {mem}MB")
            except Exception:  # noqa: BLE001
                pass
            stop.wait(4)
        c.close()

    t = threading.Thread(target=sample, daemon=True)
    t.start()
    return t


def extract(total: int) -> None:
    print(f"==> Extracting on device (~{total} entries)")
    stop = threading.Event()
    sampler = start_load_sampler(stop)
    c = connect()
    cmd = (
        f"cd /media/fat && rm -rf {REMOTE_APP} && "
        f"gzip -dc {REMOTE_TAR} | tar xvf - 2>&1 ; echo __TAR_DONE__"
    )
    # A pty makes tar's verbose output stream line-by-line instead of being
    # block-buffered, so the progress counter actually moves.
    _in, out, _err = c.exec_command(cmd, timeout=1800, get_pty=True)
    count = 0
    for raw in out:
        line = raw.strip()
        if not line:
            continue
        if "__TAR_DONE__" in line:
            break
        count += 1
        if count % 200 == 0:
            pct = min(100.0, count * 100 / total) if total else 0
            print(f"    extracted ~{count}/{total} ({pct:4.0f}%)")
    print(f"    extracted {count} entries")
    stop.set()
    sampler.join(timeout=2)
    c.close()


def finalize() -> None:
    print("==> Installing launcher + Scripts entry, verifying")
    c = connect()
    sftp = c.open_sftp()
    sftp.put(str(ENTRY), REMOTE_ENTRY)
    sftp.close()
    _in, out, _err = c.exec_command(
        f"rm -f {REMOTE_TAR}; "
        f"chmod +x {REMOTE_APP}/run-mister.sh {REMOTE_APP}/python/bin/python3.12 {REMOTE_ENTRY} 2>/dev/null; "
        f"echo '--- bundle ---'; ls -la {REMOTE_APP} | head; "
        f"echo '--- python ---'; {REMOTE_APP}/python/bin/python3.12 --version 2>&1; "
        f"echo '--- files ---'; find {REMOTE_APP} -type f | wc -l",
        timeout=120,
    )
    print(out.read().decode("utf-8", "ignore"))
    c.close()


def main() -> int:
    if not BUNDLE.is_dir():
        print(f"No bundle at {BUNDLE}; run scripts/build-arm-bundle.sh first.")
        return 1
    prune_bundle()
    total = count_entries()
    pack()
    upload()
    extract(total)
    finalize()
    print("==> Done. On the MiSTer OSD: Scripts -> mister-slint")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
