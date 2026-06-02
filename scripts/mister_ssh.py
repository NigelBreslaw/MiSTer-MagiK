#!/usr/bin/env python3
"""Reliable MiSTer SSH/SFTP helper, modelled on how mister-companion connects.

The key to a reliable connection is paramiko with password auth and both
``allow_agent`` and ``look_for_keys`` disabled - that avoids the
"Too many authentication failures" problem (client offering every agent key)
and the documented MiSTer pubkey-auth hang, without any interactive prompt.

Usage:
    python scripts/mister_ssh.py run "<command>"     # run a command, print output
    python scripts/mister_ssh.py reboot              # reboot the device
    python scripts/mister_ssh.py put <local> <remote>
    python scripts/mister_ssh.py get <remote> <local>

Environment:
    MISTER_IP    (default 192.168.1.117)
    MISTER_USER  (default root)
    MISTER_PASS  (default 1)
"""
from __future__ import annotations

import os
import sys

import paramiko


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


def run(client: paramiko.SSHClient, command: str, timeout: float = 120.0) -> int:
    stdin, stdout, stderr = client.exec_command(command, timeout=timeout, get_pty=False)
    out = stdout.read().decode("utf-8", "ignore")
    err = stderr.read().decode("utf-8", "ignore")
    rc = stdout.channel.recv_exit_status()
    if out:
        sys.stdout.write(out if out.endswith("\n") else out + "\n")
    if err.strip():
        sys.stderr.write("[stderr] " + err)
    return rc


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    action = sys.argv[1]

    if action == "reboot":
        client = connect()
        # Fire-and-forget: the connection drops as the device goes down.
        try:
            client.exec_command("nohup /sbin/reboot >/dev/null 2>&1 &", timeout=10)
        finally:
            client.close()
        print(f"reboot issued to {os.environ.get('MISTER_IP', '192.168.1.117')}")
        return 0

    if action == "run":
        if len(sys.argv) < 3:
            print("run needs a command")
            return 2
        client = connect()
        try:
            return run(client, sys.argv[2], timeout=float(os.environ.get("MISTER_CMD_TIMEOUT", "120")))
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
