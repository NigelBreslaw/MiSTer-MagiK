# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import importlib.util
import struct
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).parents[1] / "lib" / "particle_model.py"
SPEC = importlib.util.spec_from_file_location("particle_model", MODULE_PATH)
particle_model = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(particle_model)


class ParticleModelTest(unittest.TestCase):
    def fixture(self, directory: Path) -> Path:
        (directory / "fixture.mtl").write_text(
            "newmtl blue\nKd 0.0 0.5 1.0\nnewmtl red\nKd 1.0 0.1 0.1\n",
            encoding="utf-8",
        )
        path = directory / "fixture.obj"
        path.write_text(
            "\n".join(
                (
                    "mtllib fixture.mtl",
                    "v -1 0 0",
                    "v 1 0 0",
                    "v 1 2 0",
                    "v -1 2 0",
                    "v 0 1 -1",
                    "usemtl blue",
                    "f 1 2 3 4",
                    "usemtl red",
                    "f -5 -4 -1",
                )
            ),
            encoding="utf-8",
        )
        return path

    def test_triangulates_materials_and_emits_deterministic_cloud(self):
        with tempfile.TemporaryDirectory() as temporary:
            vertices, triangles, colors = particle_model.load_obj(self.fixture(Path(temporary)))
            normalized = particle_model.transform_and_normalize(vertices, "y", "-z")
            first = particle_model.encode(
                particle_model.sample_points(normalized, triangles, colors, 1024, 7)
            )
            second = particle_model.encode(
                particle_model.sample_points(normalized, triangles, colors, 1024, 7)
            )

        self.assertEqual(len(triangles), 3)
        self.assertEqual(first, second)
        magic, version, stride, count, *_ = particle_model.HEADER.unpack_from(first)
        self.assertEqual(magic, particle_model.MAGIC)
        self.assertEqual((version, stride, count), (1, 8, 1024))
        flags = [record[4] for record in struct.iter_unpack("<hhhBB", first[particle_model.HEADER.size :])]
        self.assertIn(1, flags)
        self.assertIn(2, flags)

    def test_rejects_bad_indices_and_degenerate_only_models(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bad.obj"
            path.write_text("v 0 0 0\nv 1 0 0\nf 1 2 9\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "out of bounds"):
                particle_model.load_obj(path)
            path.write_text("v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 3\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "no non-degenerate"):
                particle_model.load_obj(path)


if __name__ == "__main__":
    unittest.main()
