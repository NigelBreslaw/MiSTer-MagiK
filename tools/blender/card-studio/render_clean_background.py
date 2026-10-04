# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Render installed cards without baked studio floors, backgrounds or bloom.

Run with Blender; requires ImageMagick for linear-light alpha compositing and
resizing. Original .blend files are opened read-only and never saved. Keep the
large transparent masters outside Git; only RGB888 runtime files are installed.
"""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT_SCENES = {
    "01_arcade.rgb888": "MAGIK_01_ARCADE_CABINET",
    "02_consoles.rgb888": "MAGIK_02_CONSOLES",
    "03_computers.rgb888": "MAGIK_04_COMPUTERS",
    "04_handhelds.rgb888": "MAGIK_03_HANDHELDS",
    "05_favourites.rgb888": "MAGIK_06_FAVOURITES",
    "06_settings.rgb888": "MAGIK_05_SETTINGS",
}
STUDIOS = {
    "arcade": "MiSTer-MagiK-Arcade-Cabinet.blend",
    "root": "MiSTer-MagiK-Card-Studio.blend",
    "console": "MiSTer-MagiK-Console-Studio.blend",
    "computer": "MiSTer-MagiK-Computer-Studio.blend",
}


def scene_for(filename):
    import bpy  # type: ignore[import-not-found]

    if filename in ROOT_SCENES:
        return bpy.data.scenes[ROOT_SCENES[filename]]
    if filename.startswith("console-"):
        key = filename.removeprefix("console-").removesuffix(".rgb888")
        return bpy.data.scenes["MAGIK_CONSOLE_" + key.upper()]
    key = filename.removeprefix("computer-").removesuffix(".rgb888")
    family = key.startswith("family-")
    key = key.removeprefix("family-")
    matches = [
        scene
        for scene in bpy.data.scenes
        if scene.name.startswith("MAGIK_COMPUTER_")
        and scene.get("system_id") == key
        and (scene.get("card_kind") == "family") == family
        and (
            scene.get("licensed_geometry_imported")
            or scene.get("original_hardware_complete")
        )
    ]
    if len(matches) != 1:
        raise ValueError(
            f"Expected one reviewed scene for {filename}, found {len(matches)}"
        )
    return matches[0]


def bake_black_mask(pixels: bytes) -> bytes:
    """One LSB-first bit per row-major RGB pixel; 1 means exact (0, 0, 0).

    This describes the source, not the final card with runtime text and borders.
    It is not transparency. A renderer must prove a whole filter footprint is
    black before replacing colour work with an opaque black write.
    """
    if len(pixels) % 3:
        raise ValueError("Expected complete RGB888 pixels")
    mask = bytearray((len(pixels) // 3 + 7) // 8)
    for offset in range(0, len(pixels), 3):
        if pixels[offset : offset + 3] == b"\0\0\0":
            pixel = offset // 3
            mask[pixel // 8] |= 1 << (pixel % 8)
    return bytes(mask)


def main():
    import bpy  # type: ignore[import-not-found]

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--blend-dir", type=Path, required=True)
    parser.add_argument("--index", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--card", action="append", help="Runtime filename, repeat to select cards"
    )
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1 :])
    index = json.loads(args.index.read_text())
    files = sorted({card["file"] for card in index["cards"].values()})
    if args.card:
        if set(args.card) - set(files):
            raise ValueError("Selected card is absent from the runtime index")
        files = [name for name in files if name in args.card]
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "masters").mkdir(exist_ok=True)
    (args.output / "runtime").mkdir(exist_ok=True)
    (args.output / "masks").mkdir(exist_ok=True)
    records = []
    for group, project in STUDIOS.items():
        if group == "arcade":
            selected = [name for name in files if name == "01_arcade.rgb888"]
        elif group == "root":
            selected = [
                name
                for name in files
                if name in ROOT_SCENES and name != "01_arcade.rgb888"
            ]
        else:
            selected = [name for name in files if name.startswith(group + "-")]
        if not selected:
            continue
        # Arcade now shares the front-on cabinet pose with the Arcade hub.
        # Never silently fall back to the older angled card in the six-card studio.
        blend = (
            Path(__file__).resolve().parent / project
            if group == "arcade"
            else args.blend_dir / project
        )
        bpy.ops.wm.open_mainfile(filepath=str(blend.resolve()))
        prefs = bpy.context.preferences.addons["cycles"].preferences
        try:
            prefs.compute_device_type = "METAL"
            prefs.get_devices()
            for device in prefs.devices:
                device.use = device.type == "METAL"
            device_type = (
                "GPU" if any(d.type == "METAL" for d in prefs.devices) else "CPU"
            )
        except (TypeError, RuntimeError):
            device_type = "CPU"
        for name in selected:
            scene = scene_for(name)
            bpy.context.window.scene = scene
            hidden = []
            for collection in scene.collection.children:
                if collection.name.endswith(" | STUDIO"):
                    for obj in collection.objects:
                        if obj.type == "MESH":
                            obj.hide_render = True
                            hidden.append(obj.name)
            if not hidden:
                raise ValueError(
                    f"No explicit studio background meshes in {scene.name}"
                )
            # Preserve the hardware, cameras, materials and light rigs. Film alpha
            # removes the world from the image while retaining its illumination.
            scene.render.film_transparent = True
            scene.render.use_compositing = False
            scene.compositing_node_group = None
            scene.render.resolution_x = 1500
            scene.render.resolution_y = 2100
            scene.render.resolution_percentage = 100
            scene.render.image_settings.file_format = "PNG"
            scene.render.image_settings.color_mode = "RGBA"
            scene.render.image_settings.color_depth = "8"
            scene.cycles.device = device_type
            scene.cycles.samples = 192 if group in ("root", "arcade") else 64
            scene.cycles.use_denoising = True
            master = args.output / "masters" / (Path(name).stem + ".png")
            scene.render.filepath = str(master.resolve())
            if "FINISHED" not in bpy.ops.render.render(
                write_still=True, scene=scene.name
            ):
                raise RuntimeError(f"Render interrupted: {scene.name}")
            target = args.output / "runtime" / name
            subprocess.run(
                [
                    "magick",
                    str(master),
                    "-colorspace",
                    "RGB",
                    "-background",
                    "black",
                    "-alpha",
                    "remove",
                    "-alpha",
                    "off",
                    "-filter",
                    "Lanczos",
                    "-resize",
                    "360x504!",
                    "-colorspace",
                    "sRGB",
                    "-depth",
                    "8",
                    "RGB:" + str(target),
                ],
                check=True,
            )
            pixels = target.read_bytes()
            if len(pixels) != 360 * 504 * 3:
                raise ValueError(f"Invalid runtime raster: {target}")
            digest = hashlib.sha256(pixels).hexdigest()
            for card in index["cards"].values():
                if card["file"] == name:
                    card["sha256"] = digest
            mask = bake_black_mask(pixels)
            mask_name = "masks/" + Path(name).stem + ".black1"
            (args.output / mask_name).write_bytes(mask)
            black = sum(byte.bit_count() for byte in mask)
            records.append(
                {
                    "file": name,
                    "studio": project,
                    "scene": scene.name,
                    "hidden_backgrounds": hidden,
                    "samples": scene.cycles.samples,
                    "sha256": digest,
                    "black_fraction": black / (360 * 504),
                    "mask_file": mask_name,
                    "mask_format": "row-major-lsb-first-1-is-exact-black",
                    "mask_sha256": hashlib.sha256(mask).hexdigest(),
                }
            )
            (args.output / "render-manifest.json").write_text(
                json.dumps(records, indent=2) + "\n"
            )
            (args.output / "index.json").write_text(
                json.dumps(index, sort_keys=True, indent=2) + "\n"
            )
            print(
                f"CLEAN_CARD {name}: {black / (360 * 504):.1%} exact black", flush=True
            )


if __name__ == "__main__":
    main()
