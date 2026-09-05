#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Run bootstrap-free fast assurance for the exact commit being pushed."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from scripts.checks.repository_policy import is_classified
from scripts.magik_ci import assurance, planning, python_tests, quality

ZERO_OID = "0" * 40
CI_BOUNDARY = planning.CI_BOUNDARY


class PrePushError(Exception):
    """A deterministic pre-push rejection."""


def run_git(
    repository: Path,
    args: list[str],
    *,
    allowed_codes: tuple[int, ...] = (0,),
) -> subprocess.CompletedProcess[bytes]:
    result = subprocess.run(
        ["git", *args],
        cwd=repository,
        capture_output=True,
        check=False,
    )
    if result.returncode not in allowed_codes:
        detail = result.stderr.decode(errors="replace").strip()
        raise PrePushError(
            f"git {' '.join(args)} exited {result.returncode}"
            + (f": {detail}" if detail else "")
        )
    return result


def git_value(repository: Path, args: list[str]) -> str:
    return run_git(repository, args).stdout.decode().strip()


def require_clean_tracked_tree(repository: Path) -> None:
    for args in (
        ["diff", "--quiet", "--ignore-submodules", "--"],
        ["diff", "--cached", "--quiet", "--ignore-submodules", "--"],
    ):
        result = subprocess.run(["git", *args], cwd=repository, check=False)
        if result.returncode:
            raise PrePushError(
                "pre_push_dirty_tree: commit or restore tracked index and "
                "worktree changes before pushing"
            )


def git_paths(repository: Path, args: list[str]) -> list[str]:
    output = run_git(repository, args).stdout
    return sorted(
        value.decode(errors="surrogateescape") for value in output.split(b"\0") if value
    )


def diff_paths(repository: Path, base: str, head: str) -> list[str]:
    return git_paths(
        repository,
        ["diff", "--name-only", "-z", "--diff-filter=ACMRD", base, head],
    )


def new_branch_paths(repository: Path, remote: str, head: str) -> list[str]:
    remote_head = f"refs/remotes/{remote}/HEAD"
    default_branch = run_git(
        repository,
        ["symbolic-ref", "--quiet", remote_head],
        allowed_codes=(0, 1, 128),
    )
    if default_branch.returncode == 0:
        merge_base = run_git(
            repository,
            [
                "merge-base",
                head,
                default_branch.stdout.decode().strip(),
            ],
            allowed_codes=(0, 1, 128),
        )
        if merge_base.returncode == 0:
            return diff_paths(repository, merge_base.stdout.decode().strip(), head)
    return git_paths(repository, ["ls-tree", "--name-only", "-z", "-r", head])


def pushed_paths(repository: Path, remote: str, updates: str) -> list[str]:
    require_clean_tracked_tree(repository)
    head = git_value(repository, ["rev-parse", "HEAD"])
    paths: set[str] = set()
    branch_updates = 0
    for line_number, line in enumerate(updates.splitlines(), start=1):
        if not line.strip():
            continue
        fields = line.split()
        if len(fields) != 4:
            raise PrePushError(
                f"pre_push_invalid_update: line {line_number} must contain four fields"
            )
        local_oid, remote_ref, remote_oid = fields[1:]
        if local_oid == ZERO_OID or not remote_ref.startswith("refs/heads/"):
            continue
        branch_updates += 1
        if local_oid != head:
            raise PrePushError(
                f"pre_push_non_head: {remote_ref} points to {local_oid}, "
                f"but checked-out HEAD is {head}"
            )
        selected = (
            new_branch_paths(repository, remote, local_oid)
            if remote_oid == ZERO_OID
            else diff_paths(repository, remote_oid, local_oid)
        )
        paths.update(selected)
    return sorted(paths) if branch_updates else []


def check_classification(paths: list[str]) -> None:
    unknown = [path for path in paths if not is_classified(path)]
    if unknown:
        raise PrePushError(
            "unclassified changed paths: "
            + ", ".join(unknown)
            + "; add them to the typed impact map"
        )


def run_checks(repository: Path, paths: list[str]) -> None:
    failures: list[str] = []
    try:
        assurance.execute(repository, paths)
    except subprocess.CalledProcessError as error:
        failures.append(f"fast assurance exited {error.returncode}")
    try:
        quality.execute(repository, ["all"])
    except RuntimeError as error:
        failures.append(str(error))
    if failures:
        raise PrePushError("\n".join(failures))
    try:
        python_tests.execute(repository, paths)
    except subprocess.CalledProcessError as error:
        raise PrePushError(f"Python tests exited {error.returncode}") from error


def print_plan(
    paths: list[str] | None, repository: Path = ROOT, json_output: bool = False
) -> None:
    record = planning.report(repository, paths)
    print(json.dumps(record) if json_output else planning.render(record))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--remote", default="origin")
    parser.add_argument("--plan", action="store_true")
    parser.add_argument("--paths", nargs="*", default=None)
    parser.add_argument("--json", action="store_true", dest="json_output")
    args = parser.parse_args()
    if args.json_output and not args.plan:
        parser.error("--json requires --plan")
    return args


def main() -> int:
    args = parse_args()
    repository = args.repository.resolve()
    try:
        if args.plan:
            print_plan(args.paths, repository, args.json_output)
            return 0
        paths = pushed_paths(repository, args.remote, sys.stdin.read())
        if not paths:
            print("pre-push: no branch updates require verification")
            return 0
        check_classification(paths)
        print(f"pre-push: fast assurance for {len(paths)} changed paths")
        print(CI_BOUNDARY)
        run_checks(repository, paths)
        print("pre-push: fast gate passed; full CI remains authoritative")
        return 0
    except (PrePushError, ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
