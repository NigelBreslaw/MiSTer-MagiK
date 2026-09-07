"""Host-only Cargo maintenance and export of historical workflow evidence."""

from __future__ import annotations

import json
import sqlite3
import subprocess
from pathlib import Path


def tracked_manifest(root: Path, requested: Path) -> Path:
    root = root.resolve()
    manifest = (root / requested).resolve()
    relative = manifest.relative_to(root)
    if manifest.name != "Cargo.toml":
        raise ValueError("expected a Cargo.toml inside this repository")
    subprocess.run(
        [
            "git",
            "ls-files",
            "--error-unmatch",
            "--",
            str(relative),
            str(relative.with_name("Cargo.lock")),
        ],
        cwd=root,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    return manifest


def dependencies(root: Path, requested: Path, package: str | None = None) -> None:
    manifest = tracked_manifest(root, requested)
    cargo = str(root / "scripts/cargo")
    if package:
        subprocess.run(
            [cargo, "update", "--manifest-path", str(manifest), "--package", package],
            cwd=root,
            check=True,
            timeout=600,
        )
    else:
        # Resolve new manifest edges while retaining existing compatible lock entries.
        # An unqualified cargo update would upgrade unrelated dependencies.
        subprocess.run(
            [
                cargo,
                "metadata",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest),
            ],
            cwd=root,
            check=True,
            timeout=600,
            stdout=subprocess.DEVNULL,
        )
    subprocess.run(
        [
            cargo,
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest),
        ],
        cwd=root,
        check=True,
        timeout=600,
        stdout=subprocess.DEVNULL,
    )


def clean(root: Path, requested: Path, package: str) -> None:
    manifest = tracked_manifest(root, requested)
    if not package or package.startswith("-"):
        raise ValueError("an explicit package is required for cleaning")
    subprocess.run(
        [
            str(root / "scripts/cargo"),
            "clean",
            "--manifest-path",
            str(manifest),
            "--package",
            package,
        ],
        cwd=root,
        check=True,
        timeout=120,
    )


def export_evidence(database: Path, output: Path) -> None:
    """Export existing evidence read-only; create no new workflow database."""
    if output.exists():
        raise ValueError("refusing to overwrite an existing evidence export")
    with sqlite3.connect(
        database.resolve().as_uri() + "?mode=ro", uri=True
    ) as connection:
        connection.execute("BEGIN")
        names = [
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
            )
        ]
        result = {}
        for name in names:
            cursor = connection.execute(
                'SELECT * FROM "' + name.replace('"', '""') + '"'
            )
            result[name] = {
                "columns": [item[0] for item in cursor.description],
                "rows": [list(row) for row in cursor],
            }
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x") as stream:
        json.dump(
            result,
            stream,
            indent=2,
            default=lambda value: {"hex": value.hex()}
            if isinstance(value, bytes)
            else str(value),
        )
        stream.write("\n")
