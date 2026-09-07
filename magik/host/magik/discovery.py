"""One bounded local discovery shared by CLI, scenarios, benchmarks and MCP."""

from __future__ import annotations

import errno
import ipaddress
import os
import re
import socket
import subprocess
import time
import uuid
from concurrent.futures import ThreadPoolExecutor, wait, FIRST_COMPLETED
from dataclasses import dataclass

from .bootstrap import BootstrapError, SshBootstrap
from .device_profile import DeviceProfile, device_identity, migrate_token
from .keychain import Keychain
from .protocol import Envelope, receive_message, send_message


DISCOVERY_SECONDS = 8.0


class DiscoveryError(RuntimeError):
    pass


@dataclass(frozen=True)
class ResolvedDevice:
    identity: str
    address: str
    username: str

    def password(self) -> str:
        password = Keychain().load(self.identity, self.username)
        if password is None:
            raise DiscoveryError(
                "No SSH bootstrap password in Keychain; configure this device with device select"
            )
        return password


def native_identity(address: str, timeout: float) -> str:
    deadline = time.monotonic() + timeout
    request = Envelope(uuid.uuid4().hex, "identify", "", {})
    with socket.create_connection((address, 7500), timeout=timeout) as connection:
        connection.settimeout(timeout)
        send_message(connection, request)
        response, body = receive_message(connection, deadline=deadline)
    if (
        response.request_id != request.request_id
        or response.operation != "identified"
        or body
    ):
        raise DiscoveryError("native identification unavailable")
    return device_identity(response.fields.get("device_identity"))


def local_candidates() -> list[str]:
    """Only directly attached RFC1918 networks; cap large networks, never /16 scans."""
    addresses: list[str] = []
    for hostname in ("mister.local", "mister"):
        try:
            output = subprocess.run(
                ["/usr/bin/dscacheutil", "-q", "host", "-a", "name", hostname],
                capture_output=True,
                text=True,
                timeout=0.25,
                check=False,
            ).stdout
            addresses.extend(re.findall(r"ip_address: (\d+\.\d+\.\d+\.\d+)", output))
        except (OSError, subprocess.TimeoutExpired):
            pass
    # These fixed local commands have no credentials or remote command interface.
    for command in (["/usr/sbin/arp", "-an"], ["/sbin/ifconfig"]):
        try:
            output = subprocess.run(
                command, capture_output=True, text=True, timeout=0.5, check=False
            ).stdout
        except (OSError, subprocess.TimeoutExpired):
            continue
        if command[0].endswith("arp"):
            addresses.extend(re.findall(r"\((\d+\.\d+\.\d+\.\d+)\)", output))
        else:
            for host, mask in re.findall(
                r"inet (\d+\.\d+\.\d+\.\d+) netmask (0x[0-9a-f]+|[\d.]+)", output
            ):
                mask = (
                    str(ipaddress.IPv4Address(int(mask, 16)))
                    if mask.startswith("0x")
                    else mask
                )
                network = ipaddress.IPv4Network(f"{host}/{mask}", strict=False)
                # Limit each directly attached network to the local /24 when larger.
                if network.prefixlen < 24:
                    network = ipaddress.IPv4Network(f"{host}/24", strict=False)
                addresses.extend(str(address) for address in network.hosts())
    private = tuple(
        ipaddress.IPv4Network(value)
        for value in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")
    )
    return list(
        dict.fromkeys(
            address
            for address in addresses
            if any(ipaddress.IPv4Address(address) in net for net in private)
        )
    )[:512]


def resolve_device(
    address: str | None = None, *, select: bool = False
) -> ResolvedDevice:
    remembered = DeviceProfile.load()
    explicit = address or os.environ.get("MISTER_IP")
    username = os.environ.get("MISTER_USER") or (
        remembered.username if remembered else "root"
    )
    supplied = os.environ.get("MISTER_PASS")
    password = supplied
    deadline = time.monotonic() + DISCOVERY_SECONDS
    errors: list[Exception] = []

    def probe(candidate: str) -> ResolvedDevice | None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        try:
            identity = native_identity(candidate, min(0.4, remaining))
        except (OSError, RuntimeError, ValueError) as error:
            if isinstance(error, OSError) and error.errno in {
                errno.EPERM,
                errno.EACCES,
            }:
                raise DiscoveryError("Local-network permission denied") from error
            if password is None:
                return None
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            try:
                identity = SshBootstrap(candidate, username, password).identify(
                    min(0.7, remaining / 4)
                )
            except BootstrapError:
                raise
            except (OSError, ValueError, EOFError):
                return None
        if remembered and not select and identity != remembered.identity:
            return None
        return ResolvedDevice(identity, candidate, username)

    def accept(device: ResolvedDevice) -> ResolvedDevice:
        if supplied is not None:
            Keychain().save(device.identity, username, supplied)
        profile = DeviceProfile(device.identity, device.address, username)
        migrate_token(profile)
        profile.save()
        return device

    # Keychain is only needed for SSH, not an already reachable native service.
    initial = explicit or (remembered.address if remembered else None)
    if initial:
        try:
            ipaddress.IPv4Address(initial)
        except ValueError as error:
            raise DiscoveryError(
                "Device address must be an IPv4 address; hostname discovery is automatic"
            ) from error
        try:
            identity = native_identity(initial, 0.4)
        except (OSError, RuntimeError, ValueError):
            identity = None
        if identity is not None and (
            not remembered or select or identity == remembered.identity
        ):
            return accept(ResolvedDevice(identity, initial, username))
    if password is None and remembered:
        password = Keychain().load(remembered.identity, username)
    if initial:
        found = probe(initial)
        if found:
            return accept(found)
        if explicit:
            raise DiscoveryError(
                "Selected MiSTer is offline or has a different identity"
            )
    # Hostname resolution is a bounded local query; network probes receive only IPs.
    candidates = local_candidates()
    found: dict[str, ResolvedDevice] = {}
    pool = ThreadPoolExecutor(max_workers=32)
    pending = {
        pool.submit(probe, candidate)
        for candidate in dict.fromkeys(candidates)
        if candidate != initial
    }
    try:
        while pending and time.monotonic() < deadline:
            completed, pending = wait(
                pending,
                timeout=max(0, deadline - time.monotonic()),
                return_when=FIRST_COMPLETED,
            )
            for future in completed:
                try:
                    device = future.result()
                except Exception as error:
                    errors.append(error)
                    continue
                if device:
                    found[device.identity] = device
                    if remembered:
                        return accept(device)
        if len(found) == 1:
            return accept(next(iter(found.values())))
        if found:
            raise DiscoveryError(
                "Multiple MiSTers found; use scripts/magik device select ADDRESS: "
                + ", ".join(sorted(device.address for device in found.values()))
            )
        if errors:
            raise errors[0]
        raise DiscoveryError(
            "Remembered MiSTer is offline or no MiSTer was found within eight seconds"
        )
    finally:
        pool.shutdown(wait=False, cancel_futures=True)
