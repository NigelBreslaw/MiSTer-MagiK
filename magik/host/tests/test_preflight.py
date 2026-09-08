from types import SimpleNamespace
from unittest.mock import Mock
import subprocess
import pytest
from magik import preflight
from magik.storage import Storage


def test_no_space_reports_before_preparation(monkeypatch, tmp_path):
    monkeypatch.setattr(
        preflight.shutil, "disk_usage", lambda _: SimpleNamespace(free=100)
    )
    with pytest.raises(RuntimeError, match="Free space and rerun"):
        preflight.require_space(tmp_path / "not-created", 1024**3, "download")
    assert not (tmp_path / "not-created").exists()


def test_container_failure_retains_stderr():
    runner = Mock(
        side_effect=subprocess.CalledProcessError(
            1, ["container", "run"], stderr="internalError: mount"
        )
    )
    with pytest.raises(RuntimeError, match="internalError: mount"):
        Storage(runner=runner).command("run", "image")
