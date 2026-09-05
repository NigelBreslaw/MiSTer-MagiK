from unittest.mock import Mock
import pytest
from magik2.transfer import transfer_check
from magik2.results import create_run
from magik2.client import AgentError
from magik2.protocol import Envelope, sha256_hex


def test_transfer_retains_save_throughput_without_starting_app(tmp_path):
    artifact = tmp_path / "app"
    artifact.write_bytes(b"real app")
    agent = Mock()
    agent._request.return_value = (
        Envelope(
            "1",
            "transfer-saved",
            "",
            {
                "bytes": 8,
                "sha256": sha256_hex(b"real app"),
                "receive_ms": 2,
                "bytes_per_second": 4000,
            },
        ),
        b"",
    )
    run = create_run(tmp_path / "results", "transfer-check", {})
    assert transfer_check(agent, artifact, run) == 0
    agent._request.assert_called_once()
    agent.start.assert_not_called()
    agent.upgrade_agent.assert_not_called()
    assert '"bytes_per_second":4000' in (run / "events.jsonl").read_text()


def test_transfer_failure_is_not_retried_by_the_command(tmp_path):
    artifact = tmp_path / "app"
    artifact.write_bytes(b"real app")
    agent = Mock()
    agent._request.return_value = (
        Envelope("1", "error", "", {"code": "upload-failed"}),
        b"",
    )
    with pytest.raises(AgentError, match="upload-failed"):
        transfer_check(
            agent, artifact, create_run(tmp_path / "results", "transfer-check", {})
        )
    agent._request.assert_called_once()
    agent.start.assert_not_called()
