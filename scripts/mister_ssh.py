#!/usr/bin/env python3
"""Reliable MiSTer SSH/SFTP helper, modelled on how mister-companion connects.

The key to a reliable connection is paramiko with password auth and both
``allow_agent`` and ``look_for_keys`` disabled - that avoids the
"Too many authentication failures" problem (client offering every agent key)
and the documented MiSTer pubkey-auth hang, without any interactive prompt.

Usage:
    python scripts/mister_ssh.py run "<command>"     # run a command, print output
    python scripts/mister_ssh.py run --stream "<command>"  # stream stdout live
    python scripts/mister_ssh.py reboot              # reboot (fire-and-forget)
    python scripts/mister_ssh.py reboot-wait [secs]  # reboot, then block until back
    python scripts/mister_ssh.py wait [secs]         # block until SSH+userspace ready
    python scripts/mister_ssh.py put <local> <remote>
    python scripts/mister_ssh.py get <remote> <local>

Environment:
    MISTER_IP    (default 192.168.1.117)
    MISTER_USER  (default root)
    MISTER_PASS  (default 1)
    MISTER_CMD_TIMEOUT  seconds for run/run --stream (default 120; 0 = no limit)
"""
from __future__ import annotations

import os
import select
import socket
import sys
import time

import paramiko


def _cmd_timeout() -> float | None:
    raw = os.environ.get("MISTER_CMD_TIMEOUT", "120")
    try:
        val = float(raw)
    except ValueError:
        return 120.0
    if val <= 0:
        return None
    return val


def connect(timeout: float = 10.0) -> paramiko.SSHClient:
    host = os.environ.get("MISTER_IP", "192.168.1.117")
    user = os.environ.get("MISTER_USER", "root")
    pw = os.environ.get("MISTER_PASS", "1")
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        hostname=host,
        username=user,
        password=pw,
        timeout=timeout,
        banner_timeout=timeout,
        auth_timeout=timeout,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def run(client: paramiko.SSHClient, command: str, timeout: float | None = 120.0) -> int:
    stdin, stdout, stderr = client.exec_command(command, timeout=timeout, get_pty=False)
    out = stdout.read().decode("utf-8", "ignore")
    err = stderr.read().decode("utf-8", "ignore")
    rc = stdout.channel.recv_exit_status()
    if out:
        sys.stdout.write(out if out.endswith("\n") else out + "\n")
    if err.strip():
        sys.stderr.write("[stderr] " + err)
    return rc


def run_stream(client: paramiko.SSHClient, command: str, timeout: float | None = 120.0) -> int:
    stdin, stdout, stderr = client.exec_command(command, timeout=timeout, get_pty=False)
    channel = stdout.channel
    err_chunks: list[str] = []

    while True:
        if channel.exit_status_ready() and not channel.recv_ready() and not channel.recv_stderr_ready():
            break
        r, _, _ = select.select([channel], [], [], 0.2)
        if r:
            if channel.recv_ready():
                data = channel.recv(4096)
                if data:
                    sys.stdout.write(data.decode("utf-8", "ignore"))
                    sys.stdout.flush()
            if channel.recv_stderr_ready():
                data = channel.recv_stderr(4096)
                if data:
                    chunk = data.decode("utf-8", "ignore")
                    err_chunks.append(chunk)
                    sys.stderr.write(chunk)
                    sys.stderr.flush()
        elif channel.exit_status_ready():
            break

    rc = channel.recv_exit_status()
    if err_chunks and not err_chunks[-1].endswith("\n"):
        sys.stderr.write("\n")
    return rc


def _port_open(host: str, port: int = 22, timeout: float = 3.0) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def _userspace_ready(timeout: float = 5.0) -> str | None:
    """Return MiSTer's status line if SSH + userspace are up, else None.

    Port 22 answering only means the kernel/dropbear is alive; we additionally
    run a trivial command so we don't return before the rootfs/login works.
    """
    try:
        client = connect(timeout=timeout)
    except Exception:
        return None
    try:
        _, stdout, _ = client.exec_command("pidof MiSTer || echo BOOTING", timeout=timeout)
        out = stdout.read().decode("utf-8", "ignore").strip()
        return out or "ready"
    except Exception:
        return None
    finally:
        client.close()


def wait_down(host: str, max_seconds: float = 40.0) -> bool:
    """Block until the device stops answering on port 22 (reboot has begun)."""
    start = time.time()
    while time.time() - start < max_seconds:
        if not _port_open(host, timeout=2.0):
            print(f"  device went down after {time.time() - start:.1f}s", flush=True)
            return True
        time.sleep(1.0)
    print("  (device still answering; proceeding to wait-up anyway)", flush=True)
    return False


def wait_up(host: str, max_seconds: float = 120.0) -> int:
    """Block until SSH is ready, printing progress.

    Polls the cheap TCP port first so a still-unreachable device doesn't burn a
    multi-second SSH connect timeout each cycle (which made elapsed times wildly
    overshoot). Only once port 22 answers do we open a real session to confirm
    login works; the MiSTer userspace pid is reported as a bonus, not gated on.
    """
    start = time.time()
    attempt = 0
    while time.time() - start < max_seconds:
        attempt += 1
        elapsed = time.time() - start
        if _port_open(host, timeout=1.5):
            status = _userspace_ready(timeout=4.0)
            if status is not None:
                mister = "booting" if status == "BOOTING" else f"pid {status}"
                print(
                    f"SSH ready after {time.time() - start:.1f}s "
                    f"(attempt {attempt}); MiSTer {mister}",
                    flush=True,
                )
                return 0
        print(f"  [{elapsed:5.1f}s] waiting for ssh...", flush=True)
        time.sleep(1.0)
    print(f"TIMEOUT: device not ready after {max_seconds:.0f}s", flush=True)
    return 1


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    action = sys.argv[1]
    host = os.environ.get("MISTER_IP", "192.168.1.117")

    if action in ("reboot", "reboot-wait"):
        client = connect()
        # Fire-and-forget: the connection drops as the device goes down.
        try:
            client.exec_command("nohup /sbin/reboot >/dev/null 2>&1 &", timeout=10)
        finally:
            client.close()
        print(f"reboot issued to {host}")
        if action == "reboot-wait":
            max_seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 120.0
            wait_down(host)
            return wait_up(host, max_seconds)
        return 0

    if action == "wait":
        max_seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 120.0
        return wait_up(host, max_seconds)

    if action == "run":
        if len(sys.argv) < 3:
            print("run needs a command")
            return 2
        stream = False
        args = sys.argv[2:]
        if args and args[0] == "--stream":
            stream = True
            args = args[1:]
        if not args:
            print("run needs a command")
            return 2
        command = args[0]
        timeout = _cmd_timeout()
        client = connect()
        try:
            if stream:
                return run_stream(client, command, timeout=timeout)
            return run(client, command, timeout=timeout)
        finally:
            client.close()

    if action in ("put", "get"):
        if len(sys.argv) < 4:
            print(f"{action} needs <src> <dst>")
            return 2
        client = connect()
        try:
            sftp = client.open_sftp()
            if action == "put":
                sftp.put(sys.argv[2], sys.argv[3])
                print(f"put {sys.argv[2]} -> {sys.argv[3]}")
            else:
                sftp.get(sys.argv[2], sys.argv[3])
                print(f"get {sys.argv[2]} -> {sys.argv[3]}")
            sftp.close()
        finally:
            client.close()
        return 0

    print(f"unknown action: {action}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
