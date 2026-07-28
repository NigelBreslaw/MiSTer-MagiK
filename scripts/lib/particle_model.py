#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compile Wavefront OBJ surfaces into deterministic MiSTer particle clouds."""

from __future__ import annotations

import argparse
import math
import random
import struct
import sys
import time
from dataclasses import dataclass
from pathlib import Path

MAGIC = b"PCLOUD1\0"
VERSION = 1
STRIDE = 8
HEADER = struct.Struct("<8sHHI6h")
RECORD = struct.Struct("<hhhBB")
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


@dataclass(frozen=True)
class Point:
    xyz: tuple[float, float, float]
    palette: int
    flags: int


def _vector(value: str) -> tuple[float, float, float]:
    sign = -1.0 if value.startswith("-") else 1.0
    axis = value.removeprefix("-")
    if axis not in {"x", "y", "z"}:
        raise ValueError(f"invalid axis {value!r}")
    return tuple(sign if name == axis else 0.0 for name in ("x", "y", "z"))


def _dot(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return sum(x * y for x, y in zip(a, b))


def _cross(a: tuple[float, float, float], b: tuple[float, float, float]) -> tuple[float, float, float]:
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def _sub(a: tuple[float, float, float], b: tuple[float, float, float]) -> tuple[float, float, float]:
    return tuple(x - y for x, y in zip(a, b))


def _add(a: tuple[float, float, float], b: tuple[float, float, float]) -> tuple[float, float, float]:
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


def load_obj(path: Path) -> tuple[list[tuple[float, float, float]], list[Triangle], dict[str, tuple[float, float, float]]]:
    vertices: list[tuple[float, float, float]] = []
    triangles: list[Triangle] = []
    colors: dict[str, tuple[float, float, float]] = {}
    material = ""
    for number, line in enumerate(path.read_text(encoding="utf-8", errors="strict").splitlines(), 1):
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
                    raise ValueError(f"{path}:{number}: face index {raw} is out of bounds")
                face.append(index)
            for offset in range(1, len(face) - 1):
                triangle = Triangle((face[0], face[offset], face[offset + 1]), material)
                a, b, c = (vertices[index] for index in triangle.vertices)
                if _length(_cross(_sub(b, a), _sub(c, a))) > 1.0e-9:
                    triangles.append(triangle)
    if not vertices or not triangles:
        raise ValueError(f"{path}: no non-degenerate triangle geometry")
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
        key=lambda index: sum((rgb[channel] - PALETTE[index][channel]) ** 2 for channel in range(3)),
    )


def edge_sets(
    vertices: list[tuple[float, float, float]], triangles: list[Triangle]
) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
    uses: dict[tuple[int, int], list[tuple[tuple[float, float, float], str]]] = {}
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
            uses.setdefault(tuple(sorted((first, second))), []).append((normal, triangle.material))
    features: list[tuple[int, int]] = []
    seams: list[tuple[int, int]] = []
    for edge, adjacent in uses.items():
        if len(adjacent) == 1 or any(_dot(adjacent[0][0], other[0]) < 0.75 for other in adjacent[1:]):
            features.append(edge)
        if len({entry[1] for entry in adjacent}) > 1:
            seams.append(edge)
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
) -> list[Point]:
    rng = random.Random(seed)
    features, seams = edge_sets(vertices, triangles)
    edge_count = min(count, round(count * 0.20))
    seam_count = min(count - edge_count, round(count * 0.08)) if seams else 0
    surface_count = count - edge_count - seam_count
    points: list[Point] = []

    def append_edges(edges: list[tuple[int, int]], amount: int, flags: int) -> None:
        if not edges:
            return
        lengths = [_length(_sub(vertices[b], vertices[a])) for a, b in edges]
        total = sum(lengths)
        for _ in range(amount):
            edge = edges[_weighted_choice(rng, lengths, total)]
            value = rng.random()
            xyz = _add(_mul(vertices[edge[0]], 1.0 - value), _mul(vertices[edge[1]], value))
            points.append(Point(xyz, 7, flags))

    append_edges(features, edge_count, 1)
    append_edges(seams, seam_count, 2)
    areas: list[float] = []
    for triangle in triangles:
        a, b, c = (vertices[index] for index in triangle.vertices)
        areas.append(_length(_cross(_sub(b, a), _sub(c, a))) * 0.5)
    total_area = sum(areas)
    for _ in range(surface_count):
        triangle = triangles[_weighted_choice(rng, areas, total_area)]
        a, b, c = (vertices[index] for index in triangle.vertices)
        root = math.sqrt(rng.random())
        u, v, w = 1.0 - root, root * (1.0 - rng.random()), root * rng.random()
        xyz = _add(_add(_mul(a, u), _mul(b, v)), _mul(c, w))
        points.append(Point(xyz, palette_class(triangle.material, colors), 0))

    # Stable Morton-like ordering gives the runtime coherent formation targets and
    # thins local clumps without changing feature quotas.
    def spatial_key(point: Point) -> tuple[int, int, int, int]:
        x, y, z = point.xyz
        return (round(y * 2048), round(x * 2048), round(z * 2048), point.flags)

    points.sort(key=spatial_key)
    return points


def encode(points: list[Point]) -> bytes:
    quantized: list[tuple[int, int, int, int, int]] = []
    for point in points:
        xyz = tuple(max(-32767, min(32767, round(value * 32767.0))) for value in point.xyz)
        quantized.append((*xyz, point.palette, point.flags))
    bounds = tuple(
        function(record[axis] for record in quantized)
        for axis in range(3)
        for function in (min, max)
    )
    header = HEADER.pack(MAGIC, VERSION, STRIDE, len(quantized), *bounds)
    return header + b"".join(RECORD.pack(*record) for record in quantized)


def compile_model(args: argparse.Namespace) -> int:
    started = time.perf_counter()
    vertices, triangles, colors = load_obj(args.model)
    vertices = transform_and_normalize(vertices, args.up_axis, args.front_axis)
    points = sample_points(vertices, triangles, colors, args.points, args.seed)
    payload = encode(points)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(payload)
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    print(
        f"particle-model points={len(points)} triangles={len(triangles)} "
        f"bytes={len(payload)} elapsed_ms={elapsed_ms:.2f} output={args.output}"
    )
    return 0


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(prog="scripts/particle-model")
    commands = result.add_subparsers(dest="command", required=True)
    compile_parser = commands.add_parser("compile")
    compile_parser.add_argument("model", type=Path)
    compile_parser.add_argument("--output", type=Path, required=True)
    compile_parser.add_argument("--points", type=int, default=65_536)
    compile_parser.add_argument("--seed", type=int, default=0x1983)
    compile_parser.add_argument("--up-axis", choices=("x", "y", "z", "-x", "-y", "-z"), default="y")
    compile_parser.add_argument("--front-axis", choices=("x", "y", "z", "-x", "-y", "-z"), default="-z")
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
