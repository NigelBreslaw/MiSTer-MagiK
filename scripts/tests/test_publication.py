# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import subprocess
import tempfile
import unittest
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import delivery_tests as delivery
from scripts.magik_ci import distribution as dist
from scripts.magik_ci import manifest, publication
from scripts.tests.distribution_fixture import CandidateFixture


class FakeGitHub:
    repository = "Owner/Repo"

    def __init__(self):
        self.assets = {"beta": {"legacy-payload": b"do not replace or prune"}}
        self.revisions = {"beta": "legacy"}
        self.feeds = {}
        self.events = []
        self.fail_upload = False
        self.corrupt_download = False

    def release(self, tag):
        return (
            {"assets": [{"name": name} for name in self.assets[tag]]}
            if tag in self.assets
            else None
        )

    def revision(self, tag):
        return self.revisions[tag]

    def create(self, tag, revision):
        self.assets[tag] = {}
        self.revisions[tag] = revision
        self.events.append(("create", tag))

    def upload(self, tag, paths, replace=False):
        self.events.append(("upload", tag, replace))
        for path in paths:
            if path.name in self.assets[tag] and not replace:
                raise AssertionError("immutable overwrite attempted")
            self.assets[tag][path.name] = path.read_bytes()
            if self.fail_upload:
                raise RuntimeError("interrupted upload")

    def download(self, tag, destination, names):
        self.events.append(("verify", tag))
        for name in names:
            (destination / name).write_bytes(
                b"corrupt" if self.corrupt_download else self.assets[tag][name]
            )

    def presentation(self, tag, version, stable=False):
        self.events.append(("presentation", tag, stable))

    def feed(self, channel):
        return self.feeds.get(f"mister-magik-{channel}-db.json.zip")

    def update_feed(self, files, version):
        self.events.append(("feed", sorted(files)))
        self.feeds.update(files)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        fixture = CandidateFixture(root)
        self.stack.enter_context(patch.dict(dist.ARTWORK, fixture.artwork()))
        self.stack.enter_context(
            patch.object(
                manifest,
                "verify",
                side_effect=lambda path, *_args, **_kw: manifest.parse_fields(
                    path.read_text()
                ),
            )
        )
        self.stack.enter_context(
            patch.object(
                dist.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 0, "verified", ""),
            )
        )
        self.stack.enter_context(
            patch.dict("os.environ", {"MISTER_MAGIK_HOST_MANAGER": __file__})
        )
        self.candidate = fixture.package(channel="alpha")
        self.validated = dist.verify(
            self.candidate, channel="alpha", write_receipt=True
        )
        evidence = delivery.evidence_for_candidate(self.validated)
        evidence["execution"] = {
            "status": "passed",
            "candidate_id": self.validated["candidate_id"],
            "result_digest": delivery._results_digest(evidence["results"]),
        }
        (self.candidate / dist.EVIDENCE).write_bytes(dist.canonical_json(evidence))
        dist.write_checksums(self.candidate)
        self.github = FakeGitHub()
        self.revision = fixture.fields["magik_revision"]

    def publish(self, channel):
        return publication.publish(
            self.candidate,
            channel=channel,
            github=self.github,
            source_revision=self.revision,
        )

    def prepare(self, channel):
        publication.prepare_promotion(
            self.candidate,
            channel=channel,
            repository=self.github.repository,
            source_revision=self.revision,
            timestamp=1_700_000_000,
        )

    def test_alpha_beta_release_promote_identical_payload_and_preserve_legacy(self):
        self.publish("alpha")
        immutable = dict(self.github.assets["v0.2.42"])
        self.prepare("beta")
        self.publish("beta")
        self.prepare("release")
        self.publish("release")
        self.assertEqual(immutable, self.github.assets["v0.2.42"])
        self.assertEqual(
            self.github.assets["beta"]["legacy-payload"], b"do not replace or prune"
        )
        self.assertEqual(self.github.events[-1][0], "feed")
        self.assertEqual(len(self.github.events[-1][1]), 2)
        self.assertEqual(
            sum(event[:2] == ("upload", "v0.2.42") for event in self.github.events), 1
        )

    def test_interrupted_upload_leaves_feed_unchanged_and_retry_reconciles(self):
        self.github.fail_upload = True
        with self.assertRaisesRegex(RuntimeError, "interrupted"):
            self.publish("alpha")
        self.assertEqual(self.github.feeds, {})
        self.github.fail_upload = False
        self.publish("alpha")
        frozen = dict(self.github.assets["v0.2.42"])
        self.publish("alpha")
        self.assertEqual(frozen, self.github.assets["v0.2.42"])

    def test_remote_corruption_prevents_feed_update(self):
        self.github.corrupt_download = True
        with self.assertRaisesRegex(ValueError, "published asset differs"):
            self.publish("alpha")
        self.assertEqual(self.github.feeds, {})

    def test_same_version_different_bytes_is_not_overwritten(self):
        self.publish("alpha")
        self.github.assets["v0.2.42"][dist.asset_name(dist.LAUNCHER)] = b"different"
        before = dict(self.github.feeds)
        with self.assertRaisesRegex(ValueError, "published asset differs"):
            self.publish("alpha")
        self.assertEqual(before, self.github.feeds)
        self.assertEqual(
            self.github.assets["v0.2.42"][dist.asset_name(dist.LAUNCHER)], b"different"
        )

    def test_promotion_requires_exact_predecessor_feed_and_source(self):
        self.prepare("beta")
        with self.assertRaisesRegex(ValueError, "exact candidate in the alpha feed"):
            self.publish("beta")
        with self.assertRaisesRegex(ValueError, "source/repository"):
            publication.publish(
                self.candidate,
                channel="beta",
                github=self.github,
                source_revision="0" * 40,
            )
        self.assertEqual(self.github.events, [])

    def test_unvalidated_changed_candidate_never_writes(self):
        (self.candidate / dist.asset_name(dist.LAUNCHER)).write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.publish("alpha")
        self.assertEqual(self.github.events, [])


class GitHubBoundaryTests(unittest.TestCase):
    def test_clobber_is_restricted_to_channel_wrappers(self):
        github = publication.GitHub("Owner/Repo")
        for tag, name in [
            ("v0.2.1", "mister-magik-beta-db.json"),
            ("beta", "mister-magik--MiSTer_MagiK"),
        ]:
            with self.assertRaisesRegex(ValueError, "only channel"):
                github.upload(tag, [Path(name)], replace=True)

    def test_stable_feed_switch_is_one_nonforced_git_update(self):
        github = publication.GitHub("Owner/Repo")
        files = {
            "mister-magik-beta-db.json.zip": b"beta",
            "mister-magik-release-db.json.zip": b"release",
        }

        def api(path, data=None, method="GET"):
            if path == "git/commits/parent":
                return {"tree": {"sha": "base"}}
            return {"sha": "new"}

        with (
            patch.object(
                github, "optional", return_value={"object": {"sha": "parent"}}
            ),
            patch.object(github, "api", side_effect=api) as call,
            patch.object(
                github,
                "feed",
                side_effect=lambda channel: files[
                    f"mister-magik-{channel}-db.json.zip"
                ],
            ),
        ):
            github.update_feed(files, "0.2.42")
        changes = [
            item for item in call.call_args_list if item.args[0].startswith("git/refs")
        ]
        self.assertEqual(len(changes), 1)
        self.assertEqual(changes[0].args[1], {"sha": "new", "force": False})
        tree = next(
            item.args[1] for item in call.call_args_list if item.args[0] == "git/trees"
        )
        self.assertEqual({entry["path"] for entry in tree["tree"]}, set(files))
        self.assertEqual(tree["base_tree"], "base")


if __name__ == "__main__":
    unittest.main()
