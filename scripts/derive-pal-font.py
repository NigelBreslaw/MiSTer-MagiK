#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools==4.63.0"]
# ///
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate the renamed MiSTer MagiK pixel fonts from Press Start 2P."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import tempfile

from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont


@dataclass(frozen=True)
class FontVariant:
    filename: str
    numerator: int
    denominator: int
    family: str
    postscript_name: str


VARIANTS = (
    FontVariant(
        "MagikPixel-Regular.ttf",
        1,
        1,
        "MagiK Pixel",
        "MagikPixel-Regular",
    ),
    FontVariant(
        "MagikPixel-PAL288-Regular.ttf",
        3,
        5,
        "MagiK Pixel PAL 288",
        "MagikPixel-PAL288-Regular",
    ),
    FontVariant(
        "MagikPixel-PAL576-Regular.ttf",
        6,
        5,
        "MagiK Pixel PAL 576",
        "MagikPixel-PAL576-Regular",
    ),
)


def scaled(value: int, numerator: int, denominator: int) -> int:
    return round(value * numerator / denominator)


def set_name(font: TTFont, name_id: int, value: str) -> None:
    table = font["name"]
    table.names = [record for record in table.names if record.nameID != name_id]
    table.setName(value, name_id, 3, 1, 0x409)
    table.setName(value, name_id, 1, 0, 0)


def transformed_glyphs(font: TTFont, numerator: int, denominator: int) -> None:
    glyph_set = font.getGlyphSet()
    transform = (1.0, 0.0, 0.0, numerator / denominator, 0.0, 0.0)
    transformed = {}
    for glyph_name in font.getGlyphOrder():
        # Decompose first. Scaling a component and its referenced glyph would
        # otherwise apply the vertical transform twice.
        recording = DecomposingRecordingPen(glyph_set)
        glyph_set[glyph_name].draw(recording)
        pen = TTGlyphPen(None)
        recording.replay(TransformPen(pen, transform))
        transformed[glyph_name] = pen.glyph()

    glyf = font["glyf"]
    for glyph_name, glyph in transformed.items():
        glyf[glyph_name] = glyph
        glyph.recalcBounds(glyf)


def glyph_bounds(font: TTFont) -> tuple[int, int, int, int]:
    glyf = font["glyf"]
    bounds = [
        (glyph.xMin, glyph.yMin, glyph.xMax, glyph.yMax)
        for glyph in glyf.glyphs.values()
        if getattr(glyph, "numberOfContours", 0) != 0
    ]
    if not bounds:
        raise ValueError("font has no drawable glyphs")
    return (
        min(bound[0] for bound in bounds),
        min(bound[1] for bound in bounds),
        max(bound[2] for bound in bounds),
        max(bound[3] for bound in bounds),
    )


def set_vertical_metrics(
    font: TTFont,
    bounds: tuple[int, int, int, int],
    numerator: int,
    denominator: int,
) -> None:
    x_min, y_min, x_max, y_max = bounds
    head = font["head"]
    head.xMin = x_min
    head.yMin = y_min
    head.xMax = x_max
    head.yMax = y_max

    hhea = font["hhea"]
    hhea.ascent = y_max
    hhea.descent = y_min
    hhea.lineGap = 0

    os2 = font["OS/2"]
    os2.sTypoAscender = y_max
    os2.sTypoDescender = y_min
    os2.sTypoLineGap = 0
    os2.usWinAscent = max(0, y_max)
    os2.usWinDescent = max(0, -y_min)
    os2.fsSelection |= 1 << 7
    for field in ("sxHeight", "sCapHeight"):
        if hasattr(os2, field):
            setattr(
                os2,
                field,
                scaled(getattr(os2, field), numerator, denominator),
            )

    post = font["post"]
    post.underlinePosition = scaled(
        post.underlinePosition,
        numerator,
        denominator,
    )
    post.underlineThickness = scaled(
        post.underlineThickness,
        numerator,
        denominator,
    )


def derive(source: Path, destination: Path, variant: FontVariant) -> None:
    font = TTFont(source, recalcBBoxes=False, recalcTimestamp=False)
    transformed_glyphs(font, variant.numerator, variant.denominator)
    set_vertical_metrics(
        font,
        glyph_bounds(font),
        variant.numerator,
        variant.denominator,
    )

    set_name(font, 1, variant.family)
    set_name(font, 2, "Regular")
    set_name(
        font,
        3,
        f"{variant.family};MagiK-{variant.numerator}-{variant.denominator}",
    )
    set_name(font, 4, variant.family)
    set_name(font, 6, variant.postscript_name)
    set_name(font, 16, variant.family)
    set_name(font, 17, "Regular")
    if "DSIG" in font:
        del font["DSIG"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    font.save(destination, reorderTables=True)


def generate(source: Path, destination_dir: Path) -> None:
    for variant in VARIANTS:
        derive(source, destination_dir / variant.filename, variant)


def check(source: Path, destination_dir: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="magik-pixel-fonts-") as directory:
        candidate_dir = Path(directory)
        generate(source, candidate_dir)
        mismatches = [
            variant.filename
            for variant in VARIANTS
            if not (destination_dir / variant.filename).is_file()
            or (destination_dir / variant.filename).read_bytes()
            != (candidate_dir / variant.filename).read_bytes()
        ]
    if mismatches:
        raise SystemExit(
            "generated MagiK Pixel fonts are stale: " + ", ".join(mismatches)
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("destination_dir", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        check(args.source, args.destination_dir)
    else:
        generate(args.source, args.destination_dir)


if __name__ == "__main__":
    main()
