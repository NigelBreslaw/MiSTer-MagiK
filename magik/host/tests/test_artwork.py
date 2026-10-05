import hashlib
import json
import struct
from unittest.mock import Mock

import pytest

from magik import artwork
from magik.apps import repository
from magik.protocol import Envelope
from magik.results import create_run


def fixture(root):
    root.mkdir()
    pixels = b"p" * artwork.SOURCE_BYTES
    (root / "fixture.rgb888").write_bytes(pixels)
    index = {
        "schema": 1,
        "width": 360,
        "height": 504,
        "format": "RGB888",
        "cards": {
            key: {
                "file": "fixture.rgb888",
                "sha256": hashlib.sha256(pixels).hexdigest(),
            }
            for key in artwork.ROOT_KEYS
        },
    }
    (root / "index.json").write_text(json.dumps(index))
    return root


def ready(**fields):
    return Envelope("test", "card-artwork-ready", "", fields), b""


def test_prepared_entries_are_bundled_once_and_validated(tmp_path):
    root = fixture(tmp_path / "pack")
    pixels = b"MGCART01" + bytes(32)
    (root / "fixture.cardtex").write_bytes(pixels)
    index = json.loads((root / "index.json").read_text())
    prepared = {
        "file": "fixture.cardtex",
        "bytes": len(pixels),
        "sha256": hashlib.sha256(pixels).hexdigest(),
    }
    for source in index["cards"].values():
        source["prepared"] = prepared
    (root / "index.json").write_text(json.dumps(index))
    payload, count = artwork.bundle(root)
    size = struct.unpack(">I", payload[:4])[0]
    assert count == 2
    assert payload[4 + size : 4 + size + len(pixels)] == pixels
    (root / "fixture.cardtex").write_bytes(pixels[:-1])
    with pytest.raises(ValueError):
        artwork.bundle(root)


def test_current_pack_is_a_no_op_and_aliases_are_not_uploaded_twice(tmp_path):
    root = fixture(tmp_path / "pack")
    payload, count = artwork.bundle(root)
    length = struct.unpack(">I", payload[:4])[0]
    assert count == 1 and len(payload) == 4 + length + artwork.SOURCE_BYTES
    agent = Mock()
    agent._request.return_value = ready(sha256=hashlib.sha256(payload).hexdigest())
    assert not artwork.ensure(root, agent, create_run(tmp_path, "deploy", {}))
    assert agent._request.call_count == 1


def test_missing_pack_installs_all_files_and_reconciles_a_lost_receipt(tmp_path):
    root = fixture(tmp_path / "pack")
    payload, count = artwork.bundle(root)
    digest = hashlib.sha256(payload).hexdigest()
    for lost in (False, True):
        agent = Mock()
        agent._request.side_effect = (
            [ready(sha256=None), TimeoutError("lost receipt"), ready(sha256=digest)]
            if lost
            else [ready(sha256=None), ready(sha256=digest, files=count)]
        )
        assert artwork.ensure(root, agent, create_run(tmp_path, "deploy", {}))
        installs = [
            call
            for call in agent._request.call_args_list
            if call.args[0] == "card-artwork-install"
        ]
        assert len(installs) == 1 and installs[0].kwargs["attempts"] == 1
        assert installs[0].args[2] == payload


def test_bad_source_fails_before_any_device_request(tmp_path):
    root = fixture(tmp_path / "pack")
    (root / "fixture.rgb888").write_bytes(b"q" * artwork.SOURCE_BYTES)
    agent = Mock()
    with pytest.raises(ValueError, match="checksum"):
        artwork.ensure(root, agent, create_run(tmp_path, "deploy", {}))
    agent._request.assert_not_called()


def test_repository_bundle_keeps_approved_artwork_and_generic_console_defaults():
    root = repository() / "apps/mister/assets/ui/launcher-cards"
    payload, count = artwork.bundle(root)
    index = json.loads((root / "index.json").read_text())
    names = {source["file"] for source in index["cards"].values()} | {
        source["prepared"]["file"] for source in index["cards"].values()
    }
    assert count == len(names)
    assert len(payload) == 4 + (root / "index.json").stat().st_size + sum(
        (root / name).stat().st_size for name in names
    )
    assert all(
        key in index["cards"]
        for key in [
            "amiga",
            "c64",
            "x68000",
            "fmtowns",
            "menu:consoles:nintendo",
        ]
    )

    assert not any(name.startswith("console-") for name in names)
