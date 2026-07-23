#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Derive a renamed, vertically scaled TrueType font for a PAL raster."""

from __future__ import annotations

import argparse
from pathlib import Path

from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont


def scaled(value: int, numerator: int, denominator: int) -> int:
    return round(value * numerator / denominator)


def set_name(font: TTFont, name_id: int, value: str) -> None:
    table = font["name"]
    table.names = [record for record in table.names if record.nameID != name_id]
    table.setName(value, name_id, 3, 1, 0x409)
    table.setName(value, name_id, 1, 0, 0)


def derive(
    source: Path,
    destination: Path,
    numerator: int,
    denominator: int,
    family: str,
    postscript_name: str,
) -> None:
    font = TTFont(source, recalcBBoxes=True, recalcTimestamp=False)
    glyph_set = font.getGlyphSet()
    transformed = {}
    transform = (1.0, 0.0, 0.0, numerator / denominator, 0.0, 0.0)
    for glyph_name in font.getGlyphOrder():
        pen = TTGlyphPen(glyph_set)
        glyph_set[glyph_name].draw(TransformPen(pen, transform))
        transformed[glyph_name] = pen.glyph()
    glyf = font["glyf"]
    for glyph_name, glyph in transformed.items():
        glyf[glyph_name] = glyph

    head = font["head"]
    head.yMin = scaled(head.yMin, numerator, denominator)
    head.yMax = scaled(head.yMax, numerator, denominator)
    hhea = font["hhea"]
    for field in ("ascent", "descent", "lineGap"):
        setattr(hhea, field, scaled(getattr(hhea, field), numerator, denominator))
    os2 = font["OS/2"]
    for field in (
        "sTypoAscender",
        "sTypoDescender",
        "sTypoLineGap",
        "usWinAscent",
        "usWinDescent",
        "sxHeight",
        "sCapHeight",
    ):
        if hasattr(os2, field):
            setattr(os2, field, scaled(getattr(os2, field), numerator, denominator))
    post = font["post"]
    post.underlinePosition = scaled(post.underlinePosition, numerator, denominator)
    post.underlineThickness = scaled(post.underlineThickness, numerator, denominator)

    set_name(font, 1, family)
    set_name(font, 2, "Regular")
    set_name(font, 3, f"{family};PAL-{numerator}-{denominator}")
    set_name(font, 4, family)
    set_name(font, 6, postscript_name)
    set_name(font, 16, family)
    set_name(font, 17, "Regular")
    if "DSIG" in font:
        del font["DSIG"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    font.save(destination, reorderTables=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--numerator", type=int, required=True)
    parser.add_argument("--denominator", type=int, required=True)
    parser.add_argument("--family", required=True)
    parser.add_argument("--postscript-name", required=True)
    args = parser.parse_args()
    derive(
        args.source,
        args.destination,
        args.numerator,
        args.denominator,
        args.family,
        args.postscript_name,
    )


if __name__ == "__main__":
    main()
