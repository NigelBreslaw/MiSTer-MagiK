# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
"""Generate flat maker artwork and a dark/colour review from sourced SVG marks.

uv run --with pillow --with cairosvg python tools/brand-cards/render.py
Use --install to replace only maker/family entries in the runtime pack.
"""

import argparse
import base64
import hashlib
import json
import re
import subprocess
from xml.etree.ElementTree import register_namespace  # noqa: S405 - serialization only

from defusedxml import ElementTree as ET
from pathlib import Path

import cairosvg
from PIL import Image, ImageDraw, ImageFont

register_namespace("", "http://www.w3.org/2000/svg")

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OUT = ROOT / "outputs/brand-cards"
PACK = ROOT / "apps/mister/assets/ui/launcher-cards"
# Title is drawn by the app only when the artwork does not contain the name.
BRANDS = {
    "nintendo": ("Nintendo", "#e60012", True),
    "sega": ("Sega", "#0060a8", True),
    "atari": ("Atari", "#ac222c", False),
    "sony": ("Sony", "#17365c", True),
    "nec": ("NEC", "#1a328b", True),
    "snk": ("SNK", "#0073bb", True),
    "bandai": ("Bandai", "#ff0000", True),
    "commodore": ("Commodore", "#e5d8bc", False),
    "apple": ("Apple", "#e7e2d3", False),
    "sinclair": ("Sinclair", "#000000", True),
    "tandy": ("Tandy", "#a32832", True),
    "acorn": ("Acorn", "#174d35", True),
    "dos-pc": ("DOS / PC", "#163d40", False),
    "japanese": ("Japanese Computers", "#e7e2d3", False),
    "other": ("Other", "#322948", False),
}
GROUPS = {
    "consoles": ["atari", "sega", "sony", "nintendo", "nec", "other"],
    "handhelds": ["nintendo", "sega", "atari", "snk", "bandai", "other"],
    "computers": [
        "acorn",
        "apple",
        "commodore",
        "atari",
        "sinclair",
        "tandy",
        "dos-pc",
        "japanese",
        "other",
    ],
}


def logo(key, colour):
    name = "atari-fuji" if key == "atari" else key
    tree = ET.parse(HERE / "logos" / f"{name}.svg")
    root = tree.getroot()
    if "viewBox" not in root.attrib:
        w, h = (
            float(re.sub(r"[^0-9.]", "", root.attrib[k])) for k in ["width", "height"]
        )
        root.set("viewBox", f"0 0 {w} {h}")
    if key == "commodore":
        # Extract the original C= contours; the app supplies the name once.
        for child in list(root)[3:]:
            root.remove(child)
        root.set("viewBox", "0 0 110 105.22")
    _, _, width, height = root.attrib["viewBox"].split()
    root.set("width", width)
    root.set("height", height)
    if colour:
        for node in root.iter():
            if node.tag.split("}")[-1] in {
                "path",
                "polygon",
                "rect",
                "circle",
                "ellipse",
            }:
                node.set("fill", colour)
                node.attrib.pop("class", None)
                if "style" in node.attrib:
                    node.set(
                        "style",
                        re.sub(
                            r"fill\s*:[^;]+", f"fill:{colour}", node.attrib["style"]
                        ),
                    )
    return ET.tostring(root)


def embed(data, x, y, w, h):
    encoded = base64.b64encode(data).decode()
    return f'<image x="{x}" y="{y}" width="{w}" height="{h}" href="data:image/svg+xml;base64,{encoded}"/>'


def artwork(key, mode):
    _, brand_colour, _ = BRANDS[key]
    bg = "#000000" if mode == "dark" else brand_colour
    ink = "#f7f1e4"
    body = f'<rect width="360" height="504" fill="{bg}"/>'
    # Light paper-coloured identities retain a black metadata area for native
    # cream text. A hard edge avoids low-value gradients and dithering.
    if key in {"apple", "commodore", "japanese"} and mode == "colour":
        body += '<rect y="338" width="360" height="166" fill="#000"/>'
    if key == "nintendo":
        body += embed(logo(key, "#ed1c24" if mode == "dark" else ink), 30, 185, 300, 75)
    elif key == "sinclair":
        for n, col in enumerate(["#ec282c", "#ffd624", "#67bd38", "#079ec4"]):
            x = 43 + n * 47
            body += f'<path d="M{x} 284 L{x + 85} 111 H{x + 132} L{x + 47} 284Z" fill="{col}"/>'
        body += embed(logo(key, ink), 40, 314, 280, 43)
    elif key in {"atari", "apple", "commodore", "bandai"}:
        tint = (
            None
            if key in {"apple", "bandai"} or (key == "commodore" and mode == "colour")
            else (("#f13734" if key == "atari" else ink) if mode == "dark" else ink)
        )
        body += embed(logo(key, tint), 80, 111, 200, 207)
    elif key == "acorn":
        # Plain typography pending a reusable source for the historical logo.
        body += f'<text x="180" y="240" text-anchor="middle" font-family="Georgia" font-size="62" fill="{ink}">Acorn</text>'
    elif key == "dos-pc":
        body += f'<path d="M83 158 L143 208 L83 258 M172 259 H267" fill="none" stroke="{ink}" stroke-width="16" stroke-linejoin="miter"/>'
    elif key == "japanese":
        body += '<circle cx="180" cy="210" r="79" fill="#d72336"/>'
    elif key == "other":
        for x, y in [(102, 132), (192, 132), (102, 222), (192, 222)]:
            body += (
                f'<rect x="{x}" y="{y}" width="66" height="66" rx="10" fill="{ink}"/>'
            )
    elif key == "sega":
        # Swap the source's white backing/blue strokes for its reversed form.
        data = logo(key, None).decode()
        data = data.replace("rgb(255, 255, 255)", bg).replace("rgb(0, 96, 168)", ink)
        body += embed(data.encode(), 35, 160, 290, 110)
    else:
        body += embed(logo(key, ink), 37, 172, 286, 90)
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="360" height="504" viewBox="0 0 360 504">{body}</svg>'


def flat_colour_palette(svg):
    """Declare solid vector inks, excluding gradients and antialiased mixtures."""
    colours = []

    def colour(value):
        if re.fullmatch(r"#[0-9a-fA-F]{3}", value):
            rgb = [int(c * 2, 16) for c in value[1:]]
        elif re.fullmatch(r"#[0-9a-fA-F]{6}", value):
            rgb = [int(value[i : i + 2], 16) for i in [1, 3, 5]]
        elif match := re.fullmatch(
            r"rgb\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)", value
        ):
            rgb = [int(v) for v in match.groups()]
        else:
            return
        if max(rgb) <= 255 and rgb not in colours:
            colours.append(rgb)

    def visit(source):
        for node in ET.fromstring(source).iter():
            for field in ["fill", "stroke"]:
                colour(node.get(field, ""))
            style = node.get("style", "")
            if node.tag.split("}")[-1] == "style":
                style += node.text or ""
            for ink in re.findall(r"(?:fill|stroke)\s*[:=]\s*[\"']?([^;\"'}]+)", style):
                colour(ink.strip())
            href = node.get("href", node.get("{http://www.w3.org/1999/xlink}href", ""))
            if href.startswith("data:image/svg+xml;base64,"):
                visit(base64.b64decode(href.split(",", 1)[1]))

    visit(svg)
    if len(colours) > 16:
        raise ValueError("maker artwork exceeds the solid ink palette limit")
    return ["#" + "".join(f"{v:02x}" for v in rgb) for rgb in colours]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", action="store_true")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    sources = HERE / "cards"
    sources.mkdir(exist_ok=True)
    for mode in ["dark", "colour"]:
        dest = OUT / mode
        dest.mkdir(exist_ok=True)
        for key in BRANDS:
            svg = artwork(key, mode)
            (sources / f"{key}-{mode}.svg").write_text(svg)
            png = cairosvg.svg2png(
                bytestring=svg.encode(), output_width=1440, output_height=2016
            )
            master = dest / f"{key}-master.png"
            master.write_bytes(png)
            subprocess.run(
                [
                    "magick",
                    str(master),
                    "-colorspace",
                    "RGB",
                    "-filter",
                    "Lanczos",
                    "-resize",
                    "360x504!",
                    "-colorspace",
                    "sRGB",
                    "-depth",
                    "8",
                    str(dest / f"{key}.png"),
                ],
                check=True,
            )
            with Image.open(dest / f"{key}.png") as im:
                (dest / f"{key}.rgb888").write_bytes(im.convert("RGB").tobytes())
    if args.install:
        manifest = json.loads((PACK / "index.json").read_text())
        old = set()
        for section, keys in GROUPS.items():
            for key in keys:
                menu = f"menu:{section}:{key}"
                if menu in manifest["cards"]:
                    old.add(manifest["cards"][menu]["file"])
                pixels = (OUT / "colour" / f"{key}.rgb888").read_bytes()
                name = f"brand-{key}.rgb888"
                (PACK / name).write_bytes(pixels)
                manifest["cards"][menu] = {
                    "file": name,
                    "sha256": hashlib.sha256(pixels).hexdigest(),
                    "contains_name": BRANDS[key][2],
                    "flat_colours": flat_colour_palette(artwork(key, "colour")),
                }
        used = {s["file"] for s in manifest["cards"].values()}
        for name in old - used:
            (PACK / name).unlink()
        (PACK / "index.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n"
        )
    # Self-contained review page. Names/count labels are illustrative overlays;
    # the production renderer remains responsible for native text and frames.
    cards = []
    for key, (title, col, contains_name) in BRANDS.items():
        sides = []
        for mode in ["dark", "colour"]:
            img = base64.b64encode((OUT / mode / f"{key}.png").read_bytes()).decode()
            name = "" if contains_name else f'<div class="name">{title.upper()}</div>'
            sides.append(
                f'<div class="card"><img src="data:image/png;base64,{img}">{name}<div class="count">123 GAMES</div></div>'
            )
        note = (
            "Plain name; historical logo source is non-free." if key == "acorn" else ""
        )
        cards.append(
            f'<section><h2>{title}</h2><div class="pair">'
            + "".join(sides)
            + f"</div><p>{note}</p></section>"
        )
    html = (
        """<!doctype html><meta charset="utf-8"><title>MagiK · Maker cards</title><style>
    *{box-sizing:border-box}body{margin:0;background:#101317;color:#eee8da;font:16px system-ui}header{padding:42px 5vw 25px;border-bottom:1px solid #30343a}h1{font-size:38px;margin:0 0 12px;font-weight:550}header p{max-width:800px;color:#abb3bb;line-height:1.6}main{display:grid;grid-template-columns:repeat(auto-fit,minmax(350px,1fr));gap:40px;padding:36px 5vw}h2{font-size:16px;font-weight:500}.pair{display:flex;gap:14px}.card{width:180px;height:252px;position:relative;border:1px solid #69717d;border-radius:9px;overflow:hidden;background:black}.card img{width:100%;height:100%}.name,.count{position:absolute;width:100%;text-align:center;color:#f7f1e4;font-family:monospace}.name{top:73%;font-size:14px;font-weight:bold}.count{top:86%;font-size:11px}section p{font-size:12px;color:#a9b0b8;min-height:16px}button{border:1px solid #5c6974;background:#202934;color:#fff;padding:10px 16px;border-radius:8px;cursor:pointer}body.large .card{width:270px;height:378px}body.large main{grid-template-columns:repeat(auto-fit,minmax(555px,1fr))}body.large .name{font-size:21px}body.large .count{font-size:16px}</style><header><h1>One identity. Room to breathe.</h1><p>Maker and family cards only. Dark on the left, colour on the right. Real brand marks; flat fields; no hardware piles, floors or floating 3D text. Game totals are placeholders for this design comparison.</p><button onclick="document.body.classList.toggle('large')">Toggle viewing size</button></header><main>"""
        + "".join(cards)
        + "</main>"
    )
    (OUT / "index.html").write_text(html)
    # A contact sheet for quick visual inspection; illustrative metadata only.
    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 18)
    sheet = Image.new("RGB", (1440, 3 * 390), (16, 19, 23))
    draw = ImageDraw.Draw(sheet)
    for i, (key, (title, _, contains)) in enumerate(BRANDS.items()):
        x = (i % 5) * 288 + 24
        y = (i // 5) * 390 + 40
        im = (
            Image.open(OUT / "colour" / f"{key}.png")
            .convert("RGB")
            .resize((216, 302), Image.Resampling.LANCZOS)
        )
        sheet.paste(im, (x, y))
        draw.rounded_rectangle(
            (x, y, x + 216, y + 302), radius=7, outline=(105, 113, 125), width=1
        )
        if not contains:
            draw.text(
                (x + 108, y + 222),
                title.upper(),
                fill="#f7f1e4",
                font=font,
                anchor="mt",
            )
        draw.text(
            (x + 108, y + 262), "123 GAMES", fill="#f7f1e4", font=font, anchor="mt"
        )
        draw.text((x, y + 320), title, fill="#adb7c1", font=font)
    sheet.save(OUT / "contact-sheet.png")
    print(OUT / "index.html")


if __name__ == "__main__":
    main()
