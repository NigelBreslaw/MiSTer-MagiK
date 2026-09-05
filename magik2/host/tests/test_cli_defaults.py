from magik2 import cli


def test_everyday_check_is_real_smoke_and_prints_dev_target(
    monkeypatch, tmp_path, capsys
):
    seen = []
    monkeypatch.setenv("MISTER_MAGIK2_RESULTS", str(tmp_path))
    monkeypatch.setenv("MISTER_IP", "device")
    monkeypatch.setattr("sys.argv", ["scripts/magik2", "check"])
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
    monkeypatch.setattr("sys.argv", ["scripts/magik2", "deploy", "--app", "mini-magik"])
    monkeypatch.setattr(cli, "dispatch", lambda args, _: 2)
    assert cli.main() == 2
    output = capsys.readouterr()
    assert "/media/fat/mister-magik2/mini-magik" in output.out
    assert "/media/fat/mister-magik-dev" not in output.out
    run = next(tmp_path.iterdir())
    assert str(run / "run.json") in output.err
    assert str(run / "logs.txt") in output.err
