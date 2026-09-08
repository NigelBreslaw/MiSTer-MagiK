from magik import cli
from unittest.mock import Mock
import pytest


@pytest.mark.parametrize(
    "flags,deploys", [([], True), (["--attended"], True), (["--download-only"], False)]
)
def test_update_downloads_and_pins_exact_pair(monkeypatch, tmp_path, flags, deploys):
    from magik import updates

    pair = {"platform": {"version": 41}, "databases": {"version": 23}}
    monkeypatch.setenv("MISTER_MAGIK2_RESULTS", str(tmp_path))
    monkeypatch.setattr("sys.argv", ["scripts/magik", "update", *flags])
    download = Mock(return_value=pair)
    monkeypatch.setattr(updates, "update", download)
    monkeypatch.setattr(
        updates, "desired", Mock(side_effect=AssertionError("snapshot reread"))
    )
    dispatch = Mock(return_value=0)
    monkeypatch.setattr(cli, "dispatch", dispatch)
    assert cli.main() == 0
    download.assert_called_once_with(return_pair=True)
    assert dispatch.call_count == int(deploys)
    if deploys:
        arguments = dispatch.call_args.args[0]
        assert arguments.desired_pair is pair
        assert arguments.command == "deploy"
        assert arguments.attended == ("--attended" in flags)


def test_failed_download_never_deploys(monkeypatch):
    monkeypatch.setattr("sys.argv", ["scripts/magik", "update", "--attended"])
    monkeypatch.setattr(
        "magik.updates.update", Mock(side_effect=RuntimeError("offline"))
    )
    dispatch = Mock()
    monkeypatch.setattr(cli, "dispatch", dispatch)
    assert cli.main() == 2
    dispatch.assert_not_called()


def test_everyday_check_is_real_smoke_and_prints_dev_target(
    monkeypatch, tmp_path, capsys
):
    seen = []
    monkeypatch.setenv("MISTER_MAGIK2_RESULTS", str(tmp_path))
    monkeypatch.setenv("MISTER_IP", "device")
    monkeypatch.setattr("sys.argv", ["scripts/magik", "check"])
    monkeypatch.setattr(cli, "dispatch", lambda args, _: seen.append(args) or 0)
    assert cli.main() == 0
    assert (seen[0].app, seen[0].scenario, seen[0].profile) == ("magik", "smoke", False)
    output = capsys.readouterr().out
    assert "/media/fat/mister-magik2/magik" in output
    assert "/media/fat/mister-magik-dev" in output


def test_explicit_mini_failure_points_to_retained_evidence(
    monkeypatch, tmp_path, capsys
):
    monkeypatch.setenv("MISTER_MAGIK2_RESULTS", str(tmp_path))
    monkeypatch.setattr("sys.argv", ["scripts/magik", "deploy", "--app", "mini-magik"])
    monkeypatch.setattr(cli, "dispatch", lambda args, _: 2)
    assert cli.main() == 2
    output = capsys.readouterr()
    assert "/media/fat/mister-magik2/mini-magik" in output.out
    assert "/media/fat/mister-magik-dev" not in output.out
    run = next(tmp_path.iterdir())
    assert str(run / "run.json") in output.err
    assert str(run / "logs.txt") in output.err
