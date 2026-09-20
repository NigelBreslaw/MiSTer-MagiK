# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Rebuild the physically scaled dual-frequency molded satin finish."""

import json

import bpy  # type: ignore[import-not-found]

MAIN_SPACING_MM = 0.14
MICRO_SPACING_MM = 0.045
MAIN_RELIEF_MM = 0.024
MICRO_RELIEF_MM = 0.005
ROUGHNESS_MIN = 0.37
ROUGHNESS_MAX = 0.46


def rebuild(material: bpy.types.Material) -> dict[str, object]:
    nodes = material.node_tree.nodes
    links = material.node_tree.links
    old_texcoord = nodes.get("Texture Coordinate")
    coordinate_object = old_texcoord.object if old_texcoord else None
    if coordinate_object is None:
        raise RuntimeError(f"Metric coordinate frame missing for {material.name}")

    units_per_metre = float(material["scene_units_per_metre"])
    nodes.clear()

    texcoord = nodes.new("ShaderNodeTexCoord")
    texcoord.name = "Texture Coordinate"
    texcoord.label = "Shared metric coordinate frame"
    texcoord.object = coordinate_object

    to_mm = nodes.new("ShaderNodeVectorMath")
    to_mm.name = "Vector Math"
    to_mm.label = "Metres to millimetres"
    to_mm.operation = "SCALE"
    to_mm.inputs[3].default_value = 1000.0
    links.new(texcoord.outputs["Object"], to_mm.inputs[0])

    main_noise = nodes.new("ShaderNodeTexNoise")
    main_noise.name = "Main Grain"
    main_noise.label = f"Fine molded grain | {MAIN_SPACING_MM:g} mm"
    main_noise.noise_dimensions = "3D"
    main_noise.normalize = True
    main_noise.inputs["Scale"].default_value = 1.0 / MAIN_SPACING_MM
    main_noise.inputs["Detail"].default_value = 2.2
    main_noise.inputs["Roughness"].default_value = 0.58
    links.new(to_mm.outputs["Vector"], main_noise.inputs["Vector"])

    micro_noise = nodes.new("ShaderNodeTexNoise")
    micro_noise.name = "Micro Grain"
    micro_noise.label = f"Sub-pixel micro grain | {MICRO_SPACING_MM:g} mm"
    micro_noise.noise_dimensions = "3D"
    micro_noise.normalize = True
    micro_noise.inputs["Scale"].default_value = 1.0 / MICRO_SPACING_MM
    micro_noise.inputs["Detail"].default_value = 1.5
    micro_noise.inputs["Roughness"].default_value = 0.52
    links.new(to_mm.outputs["Vector"], micro_noise.inputs["Vector"])

    main_bump = nodes.new("ShaderNodeBump")
    main_bump.name = "Main Bump"
    main_bump.label = f"Molded relief | {MAIN_RELIEF_MM:g} mm"
    main_bump.inputs["Strength"].default_value = 0.9
    main_bump.inputs["Distance"].default_value = MAIN_RELIEF_MM / 1000 * units_per_metre
    main_bump.inputs["Filter Width"].default_value = 0.1
    links.new(main_noise.outputs["Fac"], main_bump.inputs["Height"])

    micro_bump = nodes.new("ShaderNodeBump")
    micro_bump.name = "Micro Bump"
    micro_bump.label = f"Micro relief | {MICRO_RELIEF_MM:g} mm"
    micro_bump.inputs["Strength"].default_value = 0.3
    micro_bump.inputs["Distance"].default_value = (
        MICRO_RELIEF_MM / 1000 * units_per_metre
    )
    micro_bump.inputs["Filter Width"].default_value = 0.1
    links.new(micro_noise.outputs["Fac"], micro_bump.inputs["Height"])
    links.new(main_bump.outputs["Normal"], micro_bump.inputs["Normal"])

    roughness = nodes.new("ShaderNodeMapRange")
    roughness.name = "Satin Roughness"
    roughness.label = "Fine-scale sheen breakup only"
    roughness.data_type = "FLOAT"
    roughness.clamp = True
    roughness.interpolation_type = "SMOOTHERSTEP"
    roughness.inputs["From Min"].default_value = 0.25
    roughness.inputs["From Max"].default_value = 0.75
    roughness.inputs["To Min"].default_value = ROUGHNESS_MIN
    roughness.inputs["To Max"].default_value = ROUGHNESS_MAX
    links.new(main_noise.outputs["Fac"], roughness.inputs["Value"])

    shader = nodes.new("ShaderNodeBsdfPrincipled")
    shader.name = "Principled BSDF"
    shader.label = "Satin black molded plastic"
    black = ((24 / 255 + 0.055) / 1.055) ** 2.4
    shader.inputs["Base Color"].default_value = (black, black, black, 1.0)
    shader.inputs["Metallic"].default_value = 0.0
    shader.inputs["IOR"].default_value = 1.48
    shader.inputs["Specular IOR Level"].default_value = 0.5
    shader.inputs["Coat Weight"].default_value = 0.0
    links.new(roughness.outputs["Result"], shader.inputs["Roughness"])
    links.new(micro_bump.outputs["Normal"], shader.inputs["Normal"])

    output = nodes.new("ShaderNodeOutputMaterial")
    output.name = "Material Output"
    links.new(shader.outputs["BSDF"], output.inputs["Surface"])

    positions = {
        "Texture Coordinate": (-1120, 100),
        "Vector Math": (-900, 100),
        "Main Grain": (-660, 220),
        "Micro Grain": (-660, -120),
        "Main Bump": (-380, 220),
        "Micro Bump": (-120, 80),
        "Satin Roughness": (-360, -160),
        "Principled BSDF": (180, 100),
        "Material Output": (520, 100),
    }
    for name, location in positions.items():
        nodes[name].location = location
        nodes[name].width = 220

    material.diffuse_color = (black, black, black, 1.0)
    material["recipe"] = (
        "Dual-scale satin molded plastic: 0.14 mm main grain, 0.045 mm "
        "micro grain, 0.024/0.005 mm relief, 0.37-0.46 roughness; "
        "constant sRGB #181818"
    )
    return {
        "units_per_metre": units_per_metre,
        "coordinate_object": coordinate_object.name,
        "node_count": len(nodes),
    }


def main() -> None:
    report = {
        material.name: rebuild(material)
        for material in bpy.data.materials
        if material.name.startswith("SATIN |")
        and material.name.endswith(" | main housing")
    }
    if len(report) != 6:
        raise RuntimeError(
            f"Expected six main housing materials, rebuilt {len(report)}"
        )
    bpy.ops.wm.save_as_mainfile(filepath=bpy.data.filepath)
    print("DUAL_SCALE_SATIN_APPLIED", json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    main()
