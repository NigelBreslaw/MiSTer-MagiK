#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for numbered main-only game-database promotions."""

from pathlib import Path

text = (
    Path(__file__).resolve().parents[2] / ".github/workflows/game-databases.yml"
).read_text()
trigger = text.split("on:\n", 1)[1].split("\nconcurrency:", 1)[0]
assert "  workflow_dispatch:\n" in trigger
assert "force_mame_rebuild:" in trigger
assert "type: boolean" in trigger
assert "default: false" in trigger

for required in (
    "github.ref != 'refs/heads/main'",
    "group: promote-mister-magik-game-databases",
    "cancel-in-progress: false",
    "repos/mamedev/mame/releases/latest",
    "^mame[0-9]+$",
    "^tag[0-9]+$",
    "--paginate --slurp 'repos/Robbbert/hbmame/tags?per_page=100'",
    "select-published-release.py game-databases",
    "plan-update",
    "update-needed == 'true'",
    "mame-changed == 'true'",
    "hbmame-changed == 'true'",
    "arcade-database-changed",
    "repos/MiSTer-devel/ArcadeDatabase_MiSTer/commits/main",
    "arcade-database-import",
    "ArcadeDatabase-LICENSE.txt",
    "FORCE_MAME_REBUILD",
    "mame_changed=true",
    "update_needed=true",
    "Verify Atari Lynx software list",
    "WHERE list_name='lynx'",
    "Reuse unchanged MAME database",
    "Reuse unchanged HBMAME database",
    "mame-listxml.sha256",
    "mister-magik-game-databases-v${CURRENT_VERSION}.zip",
    "game-databases-v$VERSION",
    "--draft --prerelease",
    "publish-game-databases",
    "contents: write",
):
    assert required in text, f"game-database workflow is missing: {required}"

before_publish, publish = text.split("\n  publish:\n", 1)
assert "contents: write" not in before_publish
assert "always()" in publish
assert "needs.inspect.result == 'success'" in publish
assert "needs.assemble.result == 'success'" in publish
assert "permissions:\n      actions: read\n      contents: write" in publish
assert "push:" not in trigger and "schedule:" in trigger

print("game-database workflow contract ok")
