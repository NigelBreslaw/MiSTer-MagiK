"""Fixed-purpose SSH bootstrap/repair for the otherwise-native MagiK service."""

from __future__ import annotations

import secrets
import shlex
import threading
from pathlib import Path


class BootstrapError(RuntimeError):
    """A safe, actionable bootstrap failure without credential disclosure."""


class SshBootstrap:
    """Only provisions the fixed installation layout and native service; never a shell API."""

    install_root = "/media/fat/mister-magik2"
    state_root = "/tmp/mister-magik2"

    def __init__(self, host: str, username: str, password: str) -> None:
        self.host = host
        self.username = username
        self.password = password

    @classmethod
    def from_environment(cls) -> "SshBootstrap":
        from .discovery import resolve_device

        device = resolve_device()
        return cls(device.address, device.username, device.password())

    def identify(self, timeout: float = 1.5) -> str:
        """Read board identity only; does not install or change device state."""
        import paramiko
        from .device_profile import device_identity

        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        timer = threading.Timer(timeout, client.close)
        timer.daemon = True
        timer.start()
        try:
            client.connect(
                self.host,
                username=self.username,
                password=self.password,
                timeout=timeout,
                banner_timeout=timeout,
                auth_timeout=timeout,
                look_for_keys=False,
                allow_agent=False,
            )
            _, stdout, _ = client.exec_command(
                "test -d /media/fat && test -d /sys/class/net/eth0 && cat /sys/class/net/eth0/address",
                timeout=timeout,
            )
            return device_identity(stdout.read(128).decode().strip())
        except paramiko.AuthenticationException as error:
            raise BootstrapError(
                "MiSTer SSH authentication failed; update the stored login"
            ) from error
        finally:
            timer.cancel()
            client.close()

    def install_and_start(self, agent_binary: Path) -> str:
        if not agent_binary.is_file():
            raise BootstrapError("the ARM native-agent artifact is unavailable")
        try:
            import paramiko
        except ImportError as error:  # pragma: no cover - dependency declares this
            raise BootstrapError(
                "the SSH bootstrap dependency is unavailable"
            ) from error
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        try:
            client.connect(
                self.host,
                username=self.username,
                password=self.password,
                timeout=10,
                banner_timeout=10,
                auth_timeout=10,
            )
            token = self._read_or_create_token(client)
            sftp = client.open_sftp()
            try:
                sftp.put(
                    str(agent_binary), f"{self.install_root}/mister-magik-service.next"
                )
                sftp.chmod(f"{self.install_root}/mister-magik-service.next", 0o700)
            finally:
                sftp.close()
            command = (
                f"set -eu; mkdir -p {self.install_root} {self.state_root}; "
                f"mv {self.install_root}/mister-magik-service.next {self.install_root}/mister-magik-service; "
                f"if pidof mister-magik2-agent >/dev/null; then killall mister-magik2-agent; fi; "
                f"if pidof mister-magik-service >/dev/null; then killall mister-magik-service; fi; "
                f"MISTER_MAGIK2_TOKEN={shlex.quote(token)} MISTER_MAGIK2_INSTALL_ROOT={self.install_root} MISTER_MAGIK2_STATE_ROOT={self.state_root} "
                f"nohup {self.install_root}/mister-magik-service </dev/null >{self.state_root}/agent.log 2>&1 &"
            )
            _, stdout, stderr = client.exec_command(command, timeout=15)
            if stdout.channel.recv_exit_status() != 0:
                raise BootstrapError("device rejected native-agent bootstrap")
            return token
        except BootstrapError:
            raise
        except Exception as error:
            raise BootstrapError(
                f"native-agent bootstrap failed: {type(error).__name__}"
            ) from error
        finally:
            client.close()

    def native_token(self) -> str | None:
        """Retrieve/provision only the agent token before native control traffic."""
        try:
            import paramiko
        except ImportError as error:  # pragma: no cover - dependency declares this
            raise BootstrapError(
                "the SSH bootstrap dependency is unavailable"
            ) from error
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        try:
            client.connect(
                self.host,
                username=self.username,
                password=self.password,
                timeout=10,
                banner_timeout=10,
                auth_timeout=10,
            )
            _, stdout, _ = client.exec_command(
                f"cat {self.install_root}/token 2>/dev/null || true", timeout=10
            )
            return stdout.read().decode().strip() or None
        except BootstrapError:
            raise
        except Exception as error:
            raise BootstrapError(
                f"native-agent token recovery failed: {type(error).__name__}"
            ) from error
        finally:
            client.close()

    def recent_agent_log(self) -> str:
        """Read only the fixed service log for failed-start diagnostics."""
        client = None
        try:
            import paramiko

            client = paramiko.SSHClient()
            client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
            client.connect(
                self.host,
                username=self.username,
                password=self.password,
                timeout=10,
                banner_timeout=10,
                auth_timeout=10,
            )
            _, stdout, _ = client.exec_command(
                f"tail -n 80 {self.state_root}/agent.log 2>/dev/null || true",
                timeout=10,
            )
            return stdout.read().decode(errors="replace")
        except Exception as error:
            raise BootstrapError(
                f"agent log retrieval failed: {type(error).__name__}"
            ) from error
        finally:
            if client is not None:
                client.close()

    def _read_or_create_token(self, client: object) -> str:
        token_path = f"{self.install_root}/token"
        _, stdout, _ = client.exec_command(
            f"mkdir -p {self.install_root} {self.state_root}; cat {token_path} 2>/dev/null || true",
            timeout=10,
        )
        token = stdout.read().decode().strip()
        if token:
            return token
        token = secrets.token_urlsafe(32)
        command = f"umask 077; printf %s {shlex.quote(token)} >{token_path}.next && mv {token_path}.next {token_path}"
        _, stdout, _ = client.exec_command(command, timeout=10)
        if stdout.channel.recv_exit_status() != 0:
            raise BootstrapError("could not provision native-agent credentials")
        return token
