#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check or repair SPDX headers on active first-party source files."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PROJECT_LICENSE = "GPL-3.0-or-later"

CARGO_MANIFESTS = (
    "desktop/Cargo.toml",
    "framebuffer-stream/Cargo.toml",
    "magik-gui/Cargo.toml",
    "magik-gui/catalog/Cargo.toml",
    "magik-gui/ui-generated/Cargo.toml",
    "tools/magik-agent/Cargo.toml",
    "tools/mister/Cargo.toml",
)

EXCLUDED_PREFIXES = (
    "desktop/vendor/",
    "documentation/public/screenshots/",
    "history/",
    "magik-gui/licenses/",
    "magik-gui/ui/art/",
    "magik-gui/ui/fonts/",
    "private/",
)

EXCLUDED_NAMES = {
    "LICENSE",
    "Cargo.lock",
    "pnpm-lock.yaml",
    "Menu_MiSTer-vblank-latched-fbuf.patch",
    "Menu_MiSTer.commit",
}

COMMENT_EXTENSIONS = {
    ".astro",
    ".c",
    ".cpp",
    ".css",
    ".h",
    ".mjs",
    ".py",
    ".rs",
    ".sh",
    ".slint",
    ".sv",
    ".svg",
    ".swift",
    ".toml",
    ".ts",
    ".yaml",
    ".yml",
}

HASH_COMMENT_NAMES = {
    ".gitattributes",
    ".gitignore",
    ".gitmodules",
    "Makefile",
}

TEXT_MANIFESTS = {
    "scripts/platform-component-inputs/fpga-v0.1.txt",
    "scripts/platform-component-inputs/kernel-v0.1.txt",
}

# REUSE-IgnoreStart


def tracked_files() -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    return [Path(item.decode()) for item in result.stdout.split(b"\0") if item]


def is_target(relative: Path) -> bool:
    path = relative.as_posix()
    if any(path.startswith(prefix) for prefix in EXCLUDED_PREFIXES):
        return False
    if relative.name in EXCLUDED_NAMES or relative.name.endswith(".lock"):
        return False
    if path in TEXT_MANIFESTS:
        return True
    if relative.suffix.lower() in COMMENT_EXTENSIONS:
        return True
    if relative.name in HASH_COMMENT_NAMES or relative.name.startswith("Dockerfile"):
        return True

    absolute = ROOT / relative
    try:
        with absolute.open("rb") as source:
            return source.readline().startswith(b"#!")
    except (OSError, UnicodeError):
        return False


def license_expression(relative: Path) -> str:
    return "GPL-3.0-or-later"


def comment_prefix(relative: Path) -> tuple[str, str]:
    suffix = relative.suffix.lower()
    if suffix == ".svg":
        return "<!-- ", " -->"
    if suffix == ".css":
        return "/* ", " */"
    if suffix == ".h":
        return "/* ", " */"
    if suffix in {".c", ".cpp", ".mjs", ".rs", ".slint", ".sv", ".swift", ".ts"}:
        return "// ", ""
    if suffix == ".astro":
        return "// ", ""
    return "# ", ""


def expected_lines(relative: Path) -> list[str]:
    prefix, suffix = comment_prefix(relative)
    copyright_line = f"{prefix}Copyright (C) 2026 Nigel Breslaw{suffix}\n"
    license_line = (
        f"{prefix}SPDX-License-Identifier: {license_expression(relative)}{suffix}\n"
    )
    if relative.as_posix().startswith("kernel/scanout-slots/"):
        return [license_line, copyright_line]
    return [copyright_line, license_line]


def strip_existing_header(lines: list[str]) -> list[str]:
    kept: list[str] = []
    for line in lines:
        stripped = line.strip()
        if "SPDX-License-Identifier:" in stripped:
            continue
        if "Copyright (C) 2026 Nigel Breslaw" in stripped:
            continue
        kept.append(line)
    return kept


def repaired_text(relative: Path, original: str) -> str:
    lines = strip_existing_header(original.splitlines(keepends=True))
    header = expected_lines(relative)

    if lines and lines[0].startswith("#!"):
        first = lines.pop(0)
        while lines and not lines[0].strip():
            lines.pop(0)
        return "".join([first, *header, "\n", *lines])

    if relative.suffix.lower() == ".astro" and lines and lines[0].strip() == "---":
        first = lines.pop(0)
        while lines and not lines[0].strip():
            lines.pop(0)
        return "".join([first, *header, "\n", *lines])

    while lines and not lines[0].strip():
        lines.pop(0)
    return "".join([*header, "\n", *lines])


def has_expected_header(relative: Path, text: str) -> bool:
    expected = [line.strip() for line in expected_lines(relative)]
    first_lines = [line.strip() for line in text.splitlines()[:8]]
    try:
        positions = [first_lines.index(line) for line in expected]
    except ValueError:
        return False
    return positions == sorted(positions)

# REUSE-IgnoreEnd


def metadata_errors() -> list[str]:
    errors: list[str] = []
    cargo_license = f'license = "{PROJECT_LICENSE}"'
    for relative in CARGO_MANIFESTS:
        if cargo_license not in (ROOT / relative).read_text():
            errors.append(f"{relative}: package license must be {PROJECT_LICENSE}")

    package_json = json.loads((ROOT / "documentation/package.json").read_text())
    if package_json.get("license") != PROJECT_LICENSE:
        errors.append(
            "documentation/package.json: package license must be "
            f"{PROJECT_LICENSE}"
        )

    required_license_files = (
        "COPYRIGHT",
        "LICENSES/GPL-3.0-or-later.txt",
        "REUSE.toml",
    )
    for relative in required_license_files:
        if not (ROOT / relative).is_file():
            errors.append(f"{relative}: required licensing file is missing")

    project_license = ROOT / "LICENSES/GPL-3.0-or-later.txt"
    if project_license.is_file() and (
        ROOT / "LICENSE"
    ).read_bytes() != project_license.read_bytes():
        errors.append("LICENSE: must contain the canonical GPL-3.0 license text")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--fix",
        action="store_true",
        help="insert or normalize headers instead of only checking",
    )
    args = parser.parse_args()

    targets = [
        path for path in tracked_files() if is_target(path) and (ROOT / path).is_file()
    ]
    missing: list[str] = []
    changed = 0

    for relative in targets:
        absolute = ROOT / relative
        original = absolute.read_text()
        if has_expected_header(relative, original):
            continue
        if not args.fix:
            missing.append(relative.as_posix())
            continue
        repaired = repaired_text(relative, original)
        if repaired != original:
            absolute.write_text(repaired)
            changed += 1

    errors = metadata_errors()
    if missing or errors:
        if missing:
            print("missing or incorrect license headers:", file=sys.stderr)
            for path in missing:
                print(f"  {path}", file=sys.stderr)
        if errors:
            print("incorrect licensing metadata:", file=sys.stderr)
            for error in errors:
                print(f"  {error}", file=sys.stderr)
        return 1

    action = "updated" if args.fix else "verified"
    print(f"license headers {action}: files={len(targets)} changed={changed}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
