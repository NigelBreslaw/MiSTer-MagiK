# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verify the card studio remains self-contained and renderable."""

import json

import bpy  # type: ignore[import-not-found]

EXPECTED_SCENES = {
    "MAGIK_01_ARCADE",
    "MAGIK_02_CONSOLES",
    "MAGIK_03_HANDHELDS",
    "MAGIK_04_COMPUTERS",
    "MAGIK_05_SETTINGS",
    "MAGIK_06_FAVOURITES",
}
EXPECTED_RECIPE = (
    "Dual-scale satin molded plastic: 0.14 mm main grain, 0.045 mm "
    "micro grain, 0.024/0.005 mm relief, 0.37-0.46 roughness; "
    "constant sRGB #181818"
)


def fail(message: str) -> None:
    raise RuntimeError(message)


def main() -> None:
    scenes = {
        scene.name for scene in bpy.data.scenes if scene.name.startswith("MAGIK_")
    }
    if scenes != EXPECTED_SCENES:
        fail(f"Expected scenes {sorted(EXPECTED_SCENES)}, found {sorted(scenes)}")

    images = [
        image.name
        for image in bpy.data.images
        if image.source in {"FILE", "GENERATED", "MOVIE", "SEQUENCE"}
        or image.packed_file is not None
    ]
    if images:
        fail(f"Project contains image assets: {images}")
    if bpy.data.libraries:
        fail(
            f"Project contains linked libraries: {[library.filepath for library in bpy.data.libraries]}"
        )

    text_objects = [obj.name for obj in bpy.data.objects if obj.type == "FONT"]
    if text_objects:
        fail(f"Project contains font objects: {text_objects}")
    image_empties = [
        obj.name
        for obj in bpy.data.objects
        if obj.type == "EMPTY" and getattr(obj, "data", None) is not None
    ]
    if image_empties:
        fail(f"Project contains image reference empties: {image_empties}")

    report: dict[str, object] = {"scenes": {}, "main_materials": []}
    for scene_name in sorted(EXPECTED_SCENES):
        scene = bpy.data.scenes[scene_name]
        if scene.render.engine != "CYCLES":
            fail(f"{scene_name} must use Cycles, found {scene.render.engine}")
        if (scene.render.resolution_x, scene.render.resolution_y) != (1500, 2100):
            fail(f"{scene_name} is not configured for a 1500x2100 final render")
        concept_cameras = [
            obj
            for obj in scene.objects
            if obj.type == "CAMERA" and "CONCEPT" in obj.name
        ]
        if len(concept_cameras) != 1 or scene.camera != concept_cameras[0]:
            fail(f"{scene_name} does not use exactly one CONCEPT camera")
        if scene.compositing_node_group is None:
            fail(f"{scene_name} has no compositor")
        image_nodes = [
            node.name
            for node in scene.compositing_node_group.nodes
            if node.type == "IMAGE"
        ]
        if image_nodes:
            fail(f"{scene_name} compositor contains image nodes: {image_nodes}")
        report["scenes"][scene_name] = {
            "camera": scene.camera.name,
            "objects": len(scene.objects),
            "lights": sum(obj.type == "LIGHT" for obj in scene.objects),
        }

    main_materials = [
        material
        for material in bpy.data.materials
        if material.name.startswith("SATIN |")
        and material.name.endswith(" | main housing")
    ]
    if len(main_materials) != 6:
        fail(f"Expected six satin main housing materials, found {len(main_materials)}")
    for material in main_materials:
        if material.get("recipe") != EXPECTED_RECIPE:
            fail(f"Material recipe drift in {material.name}")
        required_nodes = {
            "Main Grain",
            "Micro Grain",
            "Main Bump",
            "Micro Bump",
            "Satin Roughness",
            "Principled BSDF",
        }
        missing = required_nodes - {node.name for node in material.node_tree.nodes}
        if missing:
            fail(f"{material.name} is missing nodes: {sorted(missing)}")
        report["main_materials"].append(material.name)

    print("CARD_STUDIO_VERIFIED", json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    main()
