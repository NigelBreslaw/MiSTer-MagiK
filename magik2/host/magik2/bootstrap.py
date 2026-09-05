"""Fixed-purpose SSH bootstrap/repair for the otherwise-native MagiK 2 service."""

from __future__ import annotations

import os
import secrets
import shlex
from pathlib import Path


class BootstrapError(RuntimeError):
    """A safe, actionable bootstrap failure without credential disclosure."""


class SshBootstrap:
    """Only provisions the fixed 2.0 layout and native service; never a shell API."""

    install_root = "/media/fat/mister-magik2"
    state_root = "/tmp/mister-magik2"

    def __init__(self, host: str, username: str, password: str) -> None:
        self.host = host
        self.username = username
        self.password = password

    @classmethod
    def from_environment(cls) -> "SshBootstrap":
        missing = [name for name in ("MISTER_IP", "MISTER_USER", "MISTER_PASS") if not os.environ.get(name)]
        if missing:
            raise BootstrapError("missing configured MiSTer SSH access")
        return cls(os.environ["MISTER_IP"], os.environ["MISTER_USER"], os.environ["MISTER_PASS"])

    def install_and_start(self, agent_binary: Path) -> str:
        if not agent_binary.is_file():
            raise BootstrapError("the ARM native-agent artifact is unavailable")
        try:
            import paramiko
        except ImportError as error:  # pragma: no cover - dependency declares this
            raise BootstrapError("the SSH bootstrap dependency is unavailable") from error
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        try:
            client.connect(self.host, username=self.username, password=self.password, timeout=10, banner_timeout=10, auth_timeout=10)
            token = self._read_or_create_token(client)
            sftp = client.open_sftp()
            try:
                sftp.put(str(agent_binary), f"{self.install_root}/mister-magik2-agent.next")
                sftp.chmod(f"{self.install_root}/mister-magik2-agent.next", 0o700)
            finally:
                sftp.close()
            command = (
                f"mkdir -p {self.install_root} {self.state_root} && "
                f"mv {self.install_root}/mister-magik2-agent.next {self.install_root}/mister-magik2-agent && "
                f"pkill -f '[m]ister-magik2-agent' || true; "
                f"MISTER_MAGIK2_TOKEN={shlex.quote(token)} MISTER_MAGIK2_INSTALL_ROOT={self.install_root} "
                f"nohup {self.install_root}/mister-magik2-agent >{self.state_root}/agent.log 2>&1 &"
            )
            _, stdout, stderr = client.exec_command(command, timeout=15)
            if stdout.channel.recv_exit_status() != 0:
                raise BootstrapError("device rejected native-agent bootstrap")
            return token
        except BootstrapError:
            raise
        except Exception as error:
            raise BootstrapError(f"native-agent bootstrap failed: {type(error).__name__}") from error
        finally:
            client.close()

    def native_token(self) -> str:
        """Retrieve/provision only the agent token before native control traffic."""
        try:
            import paramiko
        except ImportError as error:  # pragma: no cover - dependency declares this
            raise BootstrapError("the SSH bootstrap dependency is unavailable") from error
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        try:
            client.connect(self.host, username=self.username, password=self.password, timeout=10, banner_timeout=10, auth_timeout=10)
            return self._read_or_create_token(client)
        except BootstrapError:
            raise
        except Exception as error:
            raise BootstrapError(f"native-agent token recovery failed: {type(error).__name__}") from error
        finally:
            client.close()

    def _read_or_create_token(self, client: object) -> str:
        token_path = f"{self.install_root}/token"
        _, stdout, _ = client.exec_command(f"mkdir -p {self.install_root} {self.state_root}; cat {token_path} 2>/dev/null || true", timeout=10)
        token = stdout.read().decode().strip()
        if token:
            return token
        token = secrets.token_urlsafe(32)
        command = f"umask 077; printf %s {shlex.quote(token)} >{token_path}.next && mv {token_path}.next {token_path}"
        _, stdout, _ = client.exec_command(command, timeout=10)
        if stdout.channel.recv_exit_status() != 0:
            raise BootstrapError("could not provision native-agent credentials")
        return token
