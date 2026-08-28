#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compile Wavefront OBJ surfaces into deterministic MiSTer particle clouds."""

from __future__ import annotations

import argparse
import json
import math
import random
import shutil
import struct
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

MAGIC = b"PCLOUD1\0"
VERSION = 1
STRIDE = 8
HEADER = struct.Struct("<8sHHI6h")
RECORD = struct.Struct("<hhhBB")
COLOR_MAGIC = b"PCOLOR1\0"
COLOR_VERSION = 1
COLOR_STRIDE = 4
COLOR_HEADER = struct.Struct("<8sHHI")
COLOR_RECORD = struct.Struct("<HH")
PALETTE = (
    (8, 8, 20),
    (30, 38, 64),
    (30, 90, 130),
    (40, 180, 210),
    (180, 45, 80),
    (240, 95, 40),
    (245, 205, 65),
    (245, 245, 245),
)


@dataclass(frozen=True)
class Triangle:
    vertices: tuple[int, int, int]
    material: str
    texture: int | None = None
    texture_factor: tuple[float, float, float] = (1.0, 1.0, 1.0)


@dataclass(frozen=True)
class Point:
    xyz: tuple[float, float, float]
    palette: int
    flags: int
    texture_exact: int
    texture_glow: int


@dataclass(frozen=True)
class TextureImage:
    width: int
    height: int
    rgb: bytes

    def __post_init__(self) -> None:
        if self.width <= 0 or self.height <= 0:
            raise ValueError("texture dimensions must be positive")
        if len(self.rgb) != self.width * self.height * 3:
            raise ValueError("texture RGB payload has the wrong length")


@dataclass(frozen=True)
class EdgeSample:
    vertices: tuple[int, int]
    triangle: Triangle


def _vector(value: str) -> tuple[float, float, float]:
    sign = -1.0 if value.startswith("-") else 1.0
    axis = value.removeprefix("-")
    if axis not in {"x", "y", "z"}:
        raise ValueError(f"invalid axis {value!r}")
    return tuple(sign if name == axis else 0.0 for name in ("x", "y", "z"))


def _dot(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return sum(x * y for x, y in zip(a, b))


def _cross(
    a: tuple[float, float, float], b: tuple[float, float, float]
) -> tuple[float, float, float]:
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def _sub(
    a: tuple[float, float, float], b: tuple[float, float, float]
) -> tuple[float, float, float]:
    return tuple(x - y for x, y in zip(a, b))


def _add(
    a: tuple[float, float, float], b: tuple[float, float, float]
) -> tuple[float, float, float]:
    return tuple(x + y for x, y in zip(a, b))


def _mul(a: tuple[float, float, float], value: float) -> tuple[float, float, float]:
    return tuple(x * value for x in a)


def _length(value: tuple[float, float, float]) -> float:
    return math.sqrt(_dot(value, value))


def load_mtl(path: Path) -> dict[str, tuple[float, float, float]]:
    colors: dict[str, tuple[float, float, float]] = {}
    current = ""
    if not path.exists():
        return colors
    for line in path.read_text(encoding="utf-8", errors="strict").splitlines():
        fields = line.split()
        if not fields or fields[0].startswith("#"):
            continue
        if fields[0] == "newmtl" and len(fields) >= 2:
            current = " ".join(fields[1:])
        elif fields[0] == "Kd" and len(fields) >= 4 and current:
            colors[current] = tuple(float(value) for value in fields[1:4])
    return colors


def load_obj(
    path: Path,
) -> tuple[
    list[tuple[float, float, float]],
    list[Triangle],
    dict[str, tuple[float, float, float]],
]:
    vertices: list[tuple[float, float, float]] = []
    triangles: list[Triangle] = []
    colors: dict[str, tuple[float, float, float]] = {}
    material = ""
    for number, line in enumerate(
        path.read_text(encoding="utf-8", errors="strict").splitlines(), 1
    ):
        fields = line.split()
        if not fields or fields[0].startswith("#"):
            continue
        if fields[0] == "mtllib":
            for name in fields[1:]:
                colors.update(load_mtl(path.parent / name))
        elif fields[0] == "usemtl":
            material = " ".join(fields[1:])
        elif fields[0] == "v":
            if len(fields) < 4:
                raise ValueError(f"{path}:{number}: vertex needs three coordinates")
            vertices.append(tuple(float(value) for value in fields[1:4]))
        elif fields[0] == "f":
            if len(fields) < 4:
                raise ValueError(f"{path}:{number}: face needs at least three vertices")
            face: list[int] = []
            for token in fields[1:]:
                raw = token.split("/", 1)[0]
                if not raw:
                    raise ValueError(f"{path}:{number}: malformed face index")
                index = int(raw)
                index = index - 1 if index > 0 else len(vertices) + index
                if index < 0 or index >= len(vertices):
                    raise ValueError(
                        f"{path}:{number}: face index {raw} is out of bounds"
                    )
                face.append(index)
            for offset in range(1, len(face) - 1):
                triangle = Triangle((face[0], face[offset], face[offset + 1]), material)
                a, b, c = (vertices[index] for index in triangle.vertices)
                if _length(_cross(_sub(b, a), _sub(c, a))) > 1.0e-9:
                    triangles.append(triangle)
    if not vertices or not triangles:
        raise ValueError(f"{path}: no non-degenerate triangle geometry")
    return vertices, triangles, colors


def _glb_accessor(
    document: dict, binary: bytes, accessor_index: int
) -> list[tuple[float, ...]]:
    accessor = document["accessors"][accessor_index]
    view = document["bufferViews"][accessor["bufferView"]]
    components = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[accessor["type"]]
    formats = {5121: "B", 5123: "H", 5125: "I", 5126: "f"}
    component_type = accessor["componentType"]
    if component_type not in formats:
        raise ValueError(f"unsupported glTF component type {component_type}")
    record = struct.Struct("<" + formats[component_type] * components)
    stride = view.get("byteStride", record.size)
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    end = start + (accessor["count"] - 1) * stride + record.size
    if end > len(binary):
        raise ValueError("glTF accessor exceeds the binary buffer")
    return [
        record.unpack_from(binary, start + index * stride)
        for index in range(accessor["count"])
    ]


def _decode_texture_image(payload: bytes) -> TextureImage:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise ValueError("baking GLB texture colours requires ffmpeg on PATH")
    try:
        completed = subprocess.run(
            (
                ffmpeg,
                "-v",
                "error",
                "-i",
                "pipe:0",
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "ppm",
                "pipe:1",
            ),
            input=payload,
            capture_output=True,
            check=False,
            timeout=30,
        )
    except subprocess.TimeoutExpired as error:
        raise ValueError("ffmpeg timed out decoding the GLB texture") from error
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        raise ValueError(f"ffmpeg cannot decode GLB texture: {detail}")
    ppm = completed.stdout
    cursor = 0

    def token() -> bytes:
        nonlocal cursor
        while cursor < len(ppm):
            if ppm[cursor] == ord("#"):
                cursor = ppm.find(b"\n", cursor)
                if cursor < 0:
                    raise ValueError("ffmpeg emitted a truncated PPM comment")
            elif chr(ppm[cursor]).isspace():
                cursor += 1
            else:
                break
        start = cursor
        while cursor < len(ppm) and not chr(ppm[cursor]).isspace():
            cursor += 1
        if start == cursor:
            raise ValueError("ffmpeg emitted a truncated PPM header")
        return ppm[start:cursor]

    if token() != b"P6":
        raise ValueError("ffmpeg did not emit a binary RGB PPM")
    width = int(token())
    height = int(token())
    if token() != b"255":
        raise ValueError("ffmpeg emitted a non-eight-bit PPM")
    if cursor >= len(ppm) or not chr(ppm[cursor]).isspace():
        raise ValueError("ffmpeg emitted a malformed PPM header")
    cursor += 1
    return TextureImage(width, height, ppm[cursor:])


def _matrix_multiply(a: list[float], b: list[float]) -> list[float]:
    result = [0.0] * 16
    for column in range(4):
        for row in range(4):
            result[column * 4 + row] = sum(
                a[index * 4 + row] * b[column * 4 + index] for index in range(4)
            )
    return result


def _transform_point(
    matrix: list[float], point: tuple[float, ...]
) -> tuple[float, float, float]:
    x, y, z = point
    divisor = matrix[3] * x + matrix[7] * y + matrix[11] * z + matrix[15]
    if abs(divisor) < 1.0e-9:
        raise ValueError("GLB node transform produced a point at infinity")
    return (
        (matrix[0] * x + matrix[4] * y + matrix[8] * z + matrix[12]) / divisor,
        (matrix[1] * x + matrix[5] * y + matrix[9] * z + matrix[13]) / divisor,
        (matrix[2] * x + matrix[6] * y + matrix[10] * z + matrix[14]) / divisor,
    )


def _load_glb(
    path: Path, decode_textures: bool
) -> tuple[
    list[tuple[float, float, float]],
    list[Triangle],
    dict[str, tuple[float, float, float]],
    list[tuple[float, float] | None],
    list[TextureImage | None],
]:
    payload = path.read_bytes()
    if len(payload) < 20 or payload[:4] != b"glTF":
        raise ValueError(f"{path}: invalid GLB header")
    version, declared_length = struct.unpack_from("<II", payload, 4)
    if version != 2 or declared_length != len(payload):
        raise ValueError(f"{path}: unsupported or truncated GLB")
    cursor = 12
    chunks: dict[int, bytes] = {}
    while cursor + 8 <= len(payload):
        length, kind = struct.unpack_from("<II", payload, cursor)
        cursor += 8
        if cursor + length > len(payload):
            raise ValueError(f"{path}: GLB chunk exceeds declared length")
        chunks[kind] = payload[cursor : cursor + length]
        cursor += length
    if 0x4E4F534A not in chunks or 0x004E4942 not in chunks:
        raise ValueError(f"{path}: GLB requires JSON and BIN chunks")
    document = json.loads(chunks[0x4E4F534A].rstrip(b" \0"))
    binary = chunks[0x004E4942]
    vertices: list[tuple[float, float, float]] = []
    triangles: list[Triangle] = []
    colors: dict[str, tuple[float, float, float]] = {}
    texture_coordinates: list[tuple[float, float] | None] = []
    materials = document.get("materials", [])
    material_textures: list[int | None] = []
    material_texture_factors: list[tuple[float, float, float]] = []
    for index, material in enumerate(materials):
        name = material.get("name", f"material-{index}")
        pbr = material.get("pbrMetallicRoughness", {})
        palette_factor = pbr.get("baseColorFactor", (0.55, 0.65, 0.8, 1.0))
        texture_factor = pbr.get("baseColorFactor", (1.0, 1.0, 1.0, 1.0))
        colors[name] = tuple(float(value) for value in palette_factor[:3])
        texture = pbr.get("baseColorTexture")
        material_textures.append(int(texture["index"]) if texture is not None else None)
        material_texture_factors.append(
            tuple(float(value) for value in texture_factor[:3])
        )
    textures: list[TextureImage | None] = [None] * len(document.get("textures", []))
    if decode_textures:
        images = document.get("images", [])
        for texture_index in {
            index for index in material_textures if index is not None
        }:
            texture = document["textures"][texture_index]
            source = images[texture["source"]]
            if "bufferView" not in source:
                raise ValueError("external GLB texture images are not supported")
            view = document["bufferViews"][source["bufferView"]]
            start = view.get("byteOffset", 0)
            end = start + view["byteLength"]
            if end > len(binary):
                raise ValueError("GLB texture image exceeds the binary buffer")
            textures[texture_index] = _decode_texture_image(binary[start:end])
    nodes = document.get("nodes", [])
    parents: dict[int, int] = {}
    for parent, node in enumerate(nodes):
        for child in node.get("children", []):
            parents[child] = parent
    identity = [
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
    world_cache: dict[int, list[float]] = {}

    def world_matrix(node_index: int) -> list[float]:
        if node_index in world_cache:
            return world_cache[node_index]
        encoded = nodes[node_index].get("matrix")
        local = (
            [float(value) for value in encoded]
            if isinstance(encoded, list) and len(encoded) == 16
            else identity
        )
        parent = parents.get(node_index)
        world = (
            local if parent is None else _matrix_multiply(world_matrix(parent), local)
        )
        world_cache[node_index] = world
        return world

    mesh_instances = [
        (node["mesh"], world_matrix(index))
        for index, node in enumerate(nodes)
        if "mesh" in node
    ]
    if not mesh_instances:
        mesh_instances = [
            (index, identity) for index in range(len(document.get("meshes", [])))
        ]
    for mesh_index, matrix in mesh_instances:
        mesh = document["meshes"][mesh_index]
        for primitive in mesh.get("primitives", []):
            if primitive.get("mode", 4) != 4:
                raise ValueError("only GLB triangle primitives are supported")
            positions = [
                _transform_point(matrix, position)
                for position in _glb_accessor(
                    document, binary, primitive["attributes"]["POSITION"]
                )
            ]
            base = len(vertices)
            vertices.extend(
                tuple(float(value) for value in position) for position in positions
            )
            encoded_uvs = primitive.get("attributes", {}).get("TEXCOORD_0")
            if encoded_uvs is None:
                texture_coordinates.extend([None] * len(positions))
            else:
                uvs = _glb_accessor(document, binary, encoded_uvs)
                if len(uvs) != len(positions):
                    raise ValueError(
                        "GLB texture-coordinate count does not match positions"
                    )
                texture_coordinates.extend((float(uv[0]), float(uv[1])) for uv in uvs)
            if "indices" in primitive:
                indices = [
                    int(value[0])
                    for value in _glb_accessor(document, binary, primitive["indices"])
                ]
            else:
                indices = list(range(len(positions)))
            if len(indices) % 3:
                raise ValueError("GLB triangle index count is not divisible by three")
            material_index = primitive.get("material")
            material = (
                materials[material_index].get("name", f"material-{material_index}")
                if material_index is not None
                else ""
            )
            texture = (
                material_textures[material_index]
                if material_index is not None
                else None
            )
            texture_factor = (
                material_texture_factors[material_index]
                if material_index is not None
                else (1.0, 1.0, 1.0)
            )
            for offset in range(0, len(indices), 3):
                face = tuple(base + indices[offset + lane] for lane in range(3))
                if any(index < base or index >= len(vertices) for index in face):
                    raise ValueError("GLB triangle index is out of bounds")
                a, b, c = (vertices[index] for index in face)
                if _length(_cross(_sub(b, a), _sub(c, a))) > 1.0e-9:
                    triangles.append(Triangle(face, material, texture, texture_factor))
    if not vertices or not triangles:
        raise ValueError(f"{path}: no non-degenerate GLB triangle geometry")
    return vertices, triangles, colors, texture_coordinates, textures


def load_glb(
    path: Path,
) -> tuple[
    list[tuple[float, float, float]],
    list[Triangle],
    dict[str, tuple[float, float, float]],
]:
    vertices, triangles, colors, _, _ = _load_glb(path, False)
    return vertices, triangles, colors


def transform_and_normalize(
    vertices: list[tuple[float, float, float]], up_axis: str, front_axis: str
) -> list[tuple[float, float, float]]:
    up = _vector(up_axis)
    front = _vector(front_axis)
    if abs(_dot(up, front)) > 0.001:
        raise ValueError("up and front axes must be perpendicular")
    right = _cross(front, up)
    transformed = [(_dot(v, right), _dot(v, up), -_dot(v, front)) for v in vertices]
    minimum = tuple(min(v[axis] for v in transformed) for axis in range(3))
    maximum = tuple(max(v[axis] for v in transformed) for axis in range(3))
    span = max(maximum[axis] - minimum[axis] for axis in range(3))
    if span <= 1.0e-9:
        raise ValueError("model has no spatial extent")
    center_x = (minimum[0] + maximum[0]) * 0.5
    center_z = (minimum[2] + maximum[2]) * 0.5
    return [
        ((x - center_x) / span, (y - minimum[1]) / span, (z - center_z) / span)
        for x, y, z in transformed
    ]


def palette_class(material: str, colors: dict[str, tuple[float, float, float]]) -> int:
    color = colors.get(material, (0.55, 0.65, 0.8))
    rgb = tuple(max(0.0, min(1.0, value)) * 255.0 for value in color)
    return min(
        range(len(PALETTE)),
        key=lambda index: sum(
            (rgb[channel] - PALETTE[index][channel]) ** 2 for channel in range(3)
        ),
    )


def rgb565(color: tuple[float, float, float]) -> int:
    red, green, blue = (round(max(0.0, min(255.0, value))) for value in color)
    return (
        ((red * 31 + 127) // 255) << 11
        | ((green * 63 + 127) // 255) << 5
        | ((blue * 31 + 127) // 255)
    )


def glow_rgb565(color: tuple[float, float, float]) -> int:
    red, green, blue = (max(0.0, min(255.0, value)) for value in color)
    luma = (54.0 * red + 183.0 * green + 19.0 * blue) / 256.0
    if luma < 1.0:
        return rgb565((16.0, 20.0, 40.0))
    scale = max(1.0, 56.0 / luma)
    return rgb565((red * scale, green * scale, blue * scale))


def sample_texture(
    texture: TextureImage, uv: tuple[float, float]
) -> tuple[float, float, float]:
    # glTF's default sampler repeats and filters linearly. Subtracting half a
    # texel matches the normalized-coordinate sampling convention at borders.
    x = (uv[0] - math.floor(uv[0])) * texture.width - 0.5
    y = (uv[1] - math.floor(uv[1])) * texture.height - 0.5
    x0 = math.floor(x)
    y0 = math.floor(y)
    x_fraction = x - x0
    y_fraction = y - y0

    def texel(pixel_x: int, pixel_y: int) -> tuple[int, int, int]:
        pixel_x %= texture.width
        pixel_y %= texture.height
        offset = (pixel_y * texture.width + pixel_x) * 3
        return tuple(texture.rgb[offset + channel] for channel in range(3))

    top_left = texel(x0, y0)
    top_right = texel(x0 + 1, y0)
    bottom_left = texel(x0, y0 + 1)
    bottom_right = texel(x0 + 1, y0 + 1)
    return tuple(
        (top_left[channel] * (1.0 - x_fraction) + top_right[channel] * x_fraction)
        * (1.0 - y_fraction)
        + (
            bottom_left[channel] * (1.0 - x_fraction)
            + bottom_right[channel] * x_fraction
        )
        * y_fraction
        for channel in range(3)
    )


def edge_sets(
    vertices: list[tuple[float, float, float]], triangles: list[Triangle]
) -> tuple[list[EdgeSample], list[EdgeSample]]:
    uses: dict[tuple[int, int], list[tuple[tuple[float, float, float], Triangle]]] = {}
    for triangle in triangles:
        a, b, c = (vertices[index] for index in triangle.vertices)
        normal = _cross(_sub(b, a), _sub(c, a))
        length = _length(normal)
        normal = _mul(normal, 1.0 / length)
        for first, second in (
            (triangle.vertices[0], triangle.vertices[1]),
            (triangle.vertices[1], triangle.vertices[2]),
            (triangle.vertices[2], triangle.vertices[0]),
        ):
            uses.setdefault(tuple(sorted((first, second))), []).append(
                (normal, triangle)
            )
    features: list[EdgeSample] = []
    seams: list[EdgeSample] = []
    for edge, adjacent in uses.items():
        if len(adjacent) == 1 or any(
            _dot(adjacent[0][0], other[0]) < 0.75 for other in adjacent[1:]
        ):
            features.append(EdgeSample(edge, adjacent[0][1]))
        if len({entry[1].material for entry in adjacent}) > 1:
            seams.append(EdgeSample(edge, adjacent[0][1]))
    return features, seams


def _weighted_choice(rng: random.Random, weights: list[float], total: float) -> int:
    target = rng.random() * total
    for index, weight in enumerate(weights):
        target -= weight
        if target <= 0.0:
            return index
    return len(weights) - 1


def sample_points(
    vertices: list[tuple[float, float, float]],
    triangles: list[Triangle],
    colors: dict[str, tuple[float, float, float]],
    count: int,
    seed: int,
    texture_coordinates: list[tuple[float, float] | None] | None = None,
    textures: list[TextureImage | None] | None = None,
) -> list[Point]:
    rng = random.Random(seed)
    features, seams = edge_sets(vertices, triangles)
    edge_count = min(count, round(count * 0.20))
    seam_count = min(count - edge_count, round(count * 0.08)) if seams else 0
    surface_count = count - edge_count - seam_count
    groups: list[list[Point]] = [[], [], []]
    seen_xyz: set[tuple[int, int, int]] = set()

    def baked_colors(color: tuple[float, float, float]) -> tuple[int, int]:
        return rgb565(color), glow_rgb565(color)

    def textured_color(
        triangle: Triangle, uv: tuple[float, float] | None
    ) -> tuple[float, float, float]:
        material_color = tuple(
            value * 255.0 for value in colors.get(triangle.material, (0.55, 0.65, 0.8))
        )
        if textures is None or triangle.texture is None or uv is None:
            return material_color
        texture = textures[triangle.texture]
        if texture is None:
            return material_color
        sampled = sample_texture(texture, uv)
        return tuple(
            sampled[channel] * triangle.texture_factor[channel] for channel in range(3)
        )

    def append_unique(
        group: list[Point], amount: int, create: Callable[[], Point]
    ) -> None:
        attempts = 0
        limit = max(10_000, amount * 100)
        while len(group) < amount:
            point = create()
            xyz = tuple(
                max(-32767, min(32767, round(value * 32767.0))) for value in point.xyz
            )
            attempts += 1
            if xyz in seen_xyz:
                if attempts >= limit:
                    raise ValueError(
                        f"cannot produce {amount} unique quantized particle points"
                    )
                continue
            seen_xyz.add(xyz)
            group.append(point)

    def append_edges(
        group: list[Point], edges: list[EdgeSample], amount: int, flags: int
    ) -> None:
        if not edges:
            return
        lengths = [
            _length(_sub(vertices[edge.vertices[1]], vertices[edge.vertices[0]]))
            for edge in edges
        ]
        total = sum(lengths)

        def create() -> Point:
            edge = edges[_weighted_choice(rng, lengths, total)]
            value = rng.random()
            first, second = edge.vertices
            xyz = _add(
                _mul(vertices[first], 1.0 - value), _mul(vertices[second], value)
            )
            uv = None
            if texture_coordinates is not None:
                first_uv = texture_coordinates[first]
                second_uv = texture_coordinates[second]
                if first_uv is not None and second_uv is not None:
                    uv = (
                        first_uv[0] * (1.0 - value) + second_uv[0] * value,
                        first_uv[1] * (1.0 - value) + second_uv[1] * value,
                    )
            exact, glow = baked_colors(textured_color(edge.triangle, uv))
            return Point(xyz, 7, flags, exact, glow)

        append_unique(group, amount, create)

    append_edges(groups[0], features, edge_count, 1)
    append_edges(groups[1], seams, seam_count, 2)
    areas: list[float] = []
    for triangle in triangles:
        a, b, c = (vertices[index] for index in triangle.vertices)
        areas.append(_length(_cross(_sub(b, a), _sub(c, a))) * 0.5)
    total_area = sum(areas)

    def create_surface() -> Point:
        triangle = triangles[_weighted_choice(rng, areas, total_area)]
        a, b, c = (vertices[index] for index in triangle.vertices)
        root = math.sqrt(rng.random())
        split = rng.random()
        u, v, w = 1.0 - root, root * (1.0 - split), root * split
        xyz = _add(_add(_mul(a, u), _mul(b, v)), _mul(c, w))
        uv = None
        if texture_coordinates is not None:
            triangle_uvs = [texture_coordinates[index] for index in triangle.vertices]
            if all(point_uv is not None for point_uv in triangle_uvs):
                first, second, third = triangle_uvs
                assert first is not None and second is not None and third is not None
                uv = (
                    first[0] * u + second[0] * v + third[0] * w,
                    first[1] * u + second[1] * v + third[1] * w,
                )
        exact, glow = baked_colors(textured_color(triangle, uv))
        return Point(xyz, palette_class(triangle.material, colors), 0, exact, glow)

    append_unique(groups[2], surface_count, create_surface)

    # Reversed Morton significance visits coarse spatial cells before refining
    # them. Interleaving the separately ordered feature/seam/surface streams
    # makes every prefix a representative, deterministic nested cloud.
    def progressive_key(point: Point) -> tuple[int, int, int, int]:
        coordinates = tuple(
            max(0, min(1023, round((value + 1.0) * 511.5))) for value in point.xyz
        )
        morton = 0
        for bit in range(10):
            for axis, value in enumerate(coordinates):
                morton |= ((value >> bit) & 1) << (bit * 3 + axis)
        reversed_morton = int(f"{morton:030b}"[::-1], 2)
        return (reversed_morton, morton, point.palette, point.flags)

    for group in groups:
        group.sort(key=progressive_key)

    points: list[Point] = []
    emitted = [0, 0, 0]
    totals = [len(group) for group in groups]
    for output_index in range(count):
        available = [
            index for index, group in enumerate(groups) if emitted[index] < len(group)
        ]
        selected = max(
            available,
            key=lambda index: (
                totals[index] * (output_index + 1) / count - emitted[index]
            ),
        )
        points.append(groups[selected][emitted[selected]])
        emitted[selected] += 1

    return points


def encode(points: list[Point]) -> bytes:
    quantized: list[tuple[int, int, int, int, int]] = []
    for point in points:
        xyz = tuple(
            max(-32767, min(32767, round(value * 32767.0))) for value in point.xyz
        )
        quantized.append((*xyz, point.palette, point.flags))
    if len({record[:3] for record in quantized}) != len(quantized):
        raise ValueError("particle cloud contains duplicate quantized coordinates")
    bounds = tuple(
        function(record[axis] for record in quantized)
        for axis in range(3)
        for function in (min, max)
    )
    header = HEADER.pack(MAGIC, VERSION, STRIDE, len(quantized), *bounds)
    return header + b"".join(RECORD.pack(*record) for record in quantized)


def encode_colors(points: list[Point]) -> bytes:
    header = COLOR_HEADER.pack(COLOR_MAGIC, COLOR_VERSION, COLOR_STRIDE, len(points))
    return header + b"".join(
        COLOR_RECORD.pack(point.texture_exact, point.texture_glow) for point in points
    )


def compile_model(args: argparse.Namespace) -> int:
    started = time.perf_counter()
    if args.model.suffix.lower() == ".glb":
        vertices, triangles, colors, texture_coordinates, textures = _load_glb(
            args.model, args.colors_output is not None
        )
    else:
        vertices, triangles, colors = load_obj(args.model)
        texture_coordinates = None
        textures = None
    vertices = transform_and_normalize(vertices, args.up_axis, args.front_axis)
    points = sample_points(
        vertices,
        triangles,
        colors,
        args.points,
        args.seed,
        texture_coordinates,
        textures,
    )
    payload = encode(points)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(payload)
    color_bytes = 0
    if args.colors_output is not None:
        color_payload = encode_colors(points)
        args.colors_output.parent.mkdir(parents=True, exist_ok=True)
        args.colors_output.write_bytes(color_payload)
        color_bytes = len(color_payload)
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    print(
        f"particle-model points={len(points)} triangles={len(triangles)} "
        f"bytes={len(payload)} color_bytes={color_bytes} elapsed_ms={elapsed_ms:.2f} "
        f"output={args.output}"
    )
    return 0


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(prog="scripts/particle-model")
    commands = result.add_subparsers(dest="command", required=True)
    compile_parser = commands.add_parser("compile")
    compile_parser.add_argument(
        "model", type=Path, help="Wavefront OBJ or binary glTF GLB"
    )
    compile_parser.add_argument("--output", type=Path, required=True)
    compile_parser.add_argument("--colors-output", type=Path)
    compile_parser.add_argument("--points", type=int, default=65_536)
    compile_parser.add_argument("--seed", type=int, default=0x1983)
    compile_parser.add_argument(
        "--up-axis", choices=("x", "y", "z", "-x", "-y", "-z"), default="y"
    )
    compile_parser.add_argument(
        "--front-axis", choices=("x", "y", "z", "-x", "-y", "-z"), default="-z"
    )
    return result


def main() -> int:
    args = parser().parse_args()
    if args.command == "compile":
        if not 1 <= args.points <= 1_000_000:
            raise ValueError("--points must be between 1 and 1000000")
        return compile_model(args)
    return 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"particle-model: {error}", file=sys.stderr)
        raise SystemExit(2) from error
