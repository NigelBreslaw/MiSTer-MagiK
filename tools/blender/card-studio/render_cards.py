# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Render the six borderless MiSTer MagiK launcher card scenes."""

import argparse
import sys
from pathlib import Path

import bpy  # type: ignore[import-not-found]

SCENE_SUFFIXES = (
    "01_ARCADE",
    "02_CONSOLES",
    "03_HANDHELDS",
    "04_COMPUTERS",
    "05_SETTINGS",
    "06_FAVOURITES",
)
QUALITY = {
    "preview": (600, 840, 48),
    "final": (1500, 2100, 192),
}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quality", choices=QUALITY, default="preview")
    parser.add_argument(
        "--scene",
        action="append",
        choices=SCENE_SUFFIXES,
        help="Scene suffix to render; repeat to select more than one.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output directory (default: renders/<quality> beside the blend file).",
    )
    script_args = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    return parser.parse_args(script_args)


def main() -> None:
    args = arguments()
    width, height, samples = QUALITY[args.quality]
    suffixes = args.scene or list(SCENE_SUFFIXES)
    project_dir = Path(bpy.data.filepath).resolve().parent
    output_dir = (args.output or project_dir / "renders" / args.quality).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    for suffix in suffixes:
        scene = bpy.data.scenes.get(f"MAGIK_{suffix}")
        if scene is None:
            raise RuntimeError(f"Missing scene MAGIK_{suffix}")
        if (
            scene.render.engine != "BLENDER_EEVEE_NEXT"
            and scene.render.engine != "CYCLES"
        ):
            raise RuntimeError(
                f"{scene.name} uses unsupported renderer {scene.render.engine}"
            )
        if any(obj.type == "FONT" for obj in scene.objects):
            raise RuntimeError(f"{scene.name} contains text objects")

        concept_cameras = [
            obj
            for obj in scene.objects
            if obj.type == "CAMERA" and "CONCEPT" in obj.name
        ]
        if len(concept_cameras) != 1:
            raise RuntimeError(
                f"{scene.name} needs exactly one CONCEPT camera, found {len(concept_cameras)}"
            )

        scene.camera = concept_cameras[0]
        scene.render.resolution_x = width
        scene.render.resolution_y = height
        scene.render.resolution_percentage = 100
        scene.render.image_settings.file_format = "PNG"
        scene.render.filepath = str(output_dir / f"{suffix}.png")
        if scene.render.engine == "CYCLES":
            scene.cycles.samples = samples
            scene.cycles.use_denoising = True

        bpy.ops.render.render(write_still=True, scene=scene.name)
        print(f"RENDERED {scene.name} -> {scene.render.filepath}")


if __name__ == "__main__":
    main()
