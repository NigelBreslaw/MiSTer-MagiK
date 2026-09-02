# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Small synthetic packages for fast Python gate tests; never qualify a release."""

import hashlib
import importlib.util
import json
import zipfile

from scripts.magik_ci import distribution as dist
from scripts.magik_ci import manifest


def load_script(name):
    spec = importlib.util.spec_from_file_location(
        name.replace("/", "_"), dist.ROOT / "scripts" / name
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CandidateFixture:
    def __init__(self, root):
        self.root = root
        self.stage = root / "stage"
        self.candidate = root / "candidate"
        self.candidate.mkdir()
        for name in dist.REQUIRED:
            target = self.stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"fixture 0.2.42\n")
        self.fields = manifest.parse_fields(
            (
                manifest.SCHEMA_PATH.parent / "generated/platform-v3.public.fixture"
            ).read_text()
        )
        self.refresh()

    def refresh(self):
        for name, value in dist.PUBLIC.items():
            if name != "root":
                self.fields[name + "_sha256"] = hashlib.sha256(
                    (self.stage / value.removeprefix("/media/fat/")).read_bytes()
                ).hexdigest()
        self.fields["qualification_candidate_id"] = manifest.candidate_id(self.fields)
        (self.stage / dist.APP / "platform-v3.manifest").write_text(
            manifest.serialize(self.fields)
        )
        (self.stage / dist.APP / "platform-bundle-v0.2.json").write_text(
            json.dumps(
                {"release_version": 16, "bundle_id": self.fields["platform_bundle_id"]}
            )
        )
        (self.stage / dist.APP / "game-databases-manifest.json").write_text(
            json.dumps({"release_version": 3})
        )
        (self.stage / dist.APP / "release-v1.txt").write_text(
            "\n".join(
                [
                    "version=0.2.42",
                    "build_number=42",
                    "game_database_version=3",
                    *(
                        f"{key}={self.fields[key]}"
                        for key in (
                            "magik_revision",
                            "main_revision",
                            "main_sha256",
                            "platform_bundle_id",
                        )
                    ),
                ]
            )
            + "\n"
        )

    def package(self, channel="beta"):
        archive = self.root / "mister-magik-0.2.42.zip"
        with zipfile.ZipFile(archive, "w") as output:
            for path in sorted(self.stage.rglob("*")):
                if path.is_file():
                    info = zipfile.ZipInfo(path.relative_to(self.stage).as_posix())
                    info.external_attr = 0o100755 << 16
                    output.writestr(info, path.read_bytes())
        packaging = load_script("release/packaging/package-release-assets.py")
        packaging.build_assets(self.stage, archive, self.root / "assets", "0.2.42", 42)
        downloader = load_script("release/databases/generate-downloader-db.py")
        downloader.generate(
            self.root / "assets/release-assets.json",
            self.root / "assets",
            channel,
            "Owner/Repo",
            "v0.2.42",
            1_700_000_000,
        )
        prepare = load_script("release/packaging/prepare-published-assets.py")
        # The caller owns this temporary fixture directory.
        self.candidate.rmdir()
        prepare.prepare(self.root / "assets", self.candidate)
        return self.candidate

    def artwork(self):
        return {
            name: hashlib.sha256((self.stage / name).read_bytes()).hexdigest()
            for name in dist.ARTWORK
        }
