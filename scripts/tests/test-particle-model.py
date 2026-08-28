# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import importlib.util
import struct
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).parents[1] / "lib" / "particle_model.py"
SPEC = importlib.util.spec_from_file_location("particle_model", MODULE_PATH)
particle_model = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = particle_model
SPEC.loader.exec_module(particle_model)


class ParticleModelTest(unittest.TestCase):
    def fixture(self, directory: Path) -> Path:
        (directory / "fixture.mtl").write_text(
            "newmtl blue\nKd 0.0 0.5 1.0\nnewmtl red\nKd 1.0 0.1 0.1\n",
            encoding="utf-8",
        )
        path = directory / "fixture.obj"
        path.write_text(
            f"""{
                chr(10).join(
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
                )
            }""",
            encoding="utf-8",
        )
        return path

    def test_triangulates_materials_and_emits_deterministic_cloud(self):
        with tempfile.TemporaryDirectory() as temporary:
            vertices, triangles, colors = particle_model.load_obj(
                self.fixture(Path(temporary))
            )
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
        flags = [
            record[4]
            for record in struct.iter_unpack(
                "<hhhBB", first[particle_model.HEADER.size :]
            )
        ]
        self.assertIn(1, flags)
        self.assertIn(2, flags)
        records = list(
            struct.iter_unpack("<hhhBB", first[particle_model.HEADER.size :])
        )
        self.assertEqual(len({record[:3] for record in records}), 1024)

        for prefix in (64, 256, 1024):
            prefix_flags = [record[4] for record in records[:prefix]]
            self.assertGreater(sum(flag == 1 for flag in prefix_flags), prefix * 0.15)
            self.assertGreater(sum(flag == 2 for flag in prefix_flags), prefix * 0.05)

    def test_rejects_bad_indices_and_degenerate_only_models(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bad.obj"
            path.write_text("v 0 0 0\nv 1 0 0\nf 1 2 9\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "out of bounds"):
                particle_model.load_obj(path)
            path.write_text("v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 3\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "no non-degenerate"):
                particle_model.load_obj(path)

    def test_surface_samples_remain_inside_source_triangle(self):
        vertices = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]
        triangles = [particle_model.Triangle((0, 1, 2), "")]
        points = particle_model.sample_points(vertices, triangles, {}, 4096, 19)

        self.assertEqual(len(points), 4096)
        for point in points:
            x, y, z = point.xyz
            self.assertGreaterEqual(x, 0.0)
            self.assertGreaterEqual(y, 0.0)
            self.assertLessEqual(x + y, 1.0 + 1.0e-7)
            self.assertAlmostEqual(z, 0.0)

    def test_texture_sampling_repeats_and_filters_at_texel_centres(self):
        texture = particle_model.TextureImage(
            2,
            2,
            bytes(
                (
                    255,
                    0,
                    0,
                    0,
                    255,
                    0,
                    0,
                    0,
                    255,
                    255,
                    255,
                    255,
                )
            ),
        )

        self.assertEqual(
            particle_model.sample_texture(texture, (0.25, 0.25)),
            (255.0, 0.0, 0.0),
        )
        self.assertEqual(
            particle_model.sample_texture(texture, (1.75, -0.25)),
            (255.0, 255.0, 255.0),
        )
        self.assertEqual(
            particle_model.sample_texture(texture, (0.5, 0.25)),
            (127.5, 127.5, 0.0),
        )

    def test_particle_colour_sidecar_is_fixed_width_rgb565(self):
        point = particle_model.Point(
            (0.0, 0.0, 0.0),
            0,
            0,
            particle_model.rgb565((255.0, 0.0, 0.0)),
            particle_model.glow_rgb565((8.0, 4.0, 2.0)),
        )
        encoded = particle_model.encode_colors([point])

        self.assertEqual(len(encoded), particle_model.COLOR_HEADER.size + 4)
        self.assertEqual(
            particle_model.COLOR_HEADER.unpack_from(encoded),
            (particle_model.COLOR_MAGIC, 1, 4, 1),
        )
        exact, glow = particle_model.COLOR_RECORD.unpack_from(
            encoded, particle_model.COLOR_HEADER.size
        )
        self.assertEqual(exact, 0xF800)
        self.assertNotEqual(glow, exact)


if __name__ == "__main__":
    unittest.main()
