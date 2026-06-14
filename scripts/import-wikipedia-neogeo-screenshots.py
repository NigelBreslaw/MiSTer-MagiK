#!/usr/bin/env python3
"""Prototype-only Wikipedia Neo Geo screenshot importer."""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import sys
import time
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

API = "https://en.wikipedia.org/w/api.php"
CATEGORY = "Category:Screenshots_of_Neo_Geo_games"


@dataclass(frozen=True)
class Identity:
    setname: str
    title: str
    keys: tuple[str, ...]


def normalize_title(value: str) -> str:
    value = urllib.parse.unquote(value)
    value = value.replace("_", " ")
    value = re.sub(r"(?i)^file:", "", value)
    value = re.sub(r"\.[a-z0-9]+$", "", value)
    value = re.sub(r"(?i)\b(neogeo|neo geo|screenshot|title screen|arcade)\b", " ", value)
    value = re.sub(r"\([^)]*\)", " ", value)
    value = value.replace("&", " and ")
    value = value.replace("’", "'")
    value = re.sub(r"[^a-zA-Z0-9']+", " ", value)
    value = re.sub(r"\b(the|a|an)\b", " ", value, flags=re.IGNORECASE)
    value = re.sub(r"\s+", " ", value).strip().lower()
    return value


def title_keys(title: str) -> tuple[str, ...]:
    parts = [title]
    for sep in ["/", ":", "("]:
        more: list[str] = []
        for part in parts:
            more.extend(part.split(sep))
        parts = more
    keys = {normalize_title(part) for part in parts}
    keys.add(normalize_title(title))
    return tuple(sorted(key for key in keys if key))


def load_identities(path: Path) -> list[Identity]:
    conn = sqlite3.connect(path)
    try:
        rows = conn.execute(
            """
            SELECT setname,title
            FROM mame_machines
            WHERE sourcefile LIKE '%neogeo%'
               OR setname IN ('mslug3','kof98','aof2')
            ORDER BY setname
            """
        ).fetchall()
    finally:
        conn.close()
    return [Identity(setname=row[0], title=row[1], keys=title_keys(row[1])) for row in rows]


def map_filename(filename: str, identities: list[Identity]) -> tuple[str, str]:
    key = normalize_title(filename)
    exact = [identity for identity in identities if key in identity.keys]
    if len(exact) == 1:
        return exact[0].setname, "mapped"
    if len(exact) > 1:
        return ",".join(identity.setname for identity in exact), "ambiguous"

    fuzzy = []
    for identity in identities:
        for identity_key in identity.keys:
            if key and identity_key and (key in identity_key or identity_key in key):
                fuzzy.append(identity)
                break
    fuzzy = sorted({identity.setname: identity for identity in fuzzy}.values(), key=lambda item: item.setname)
    if len(fuzzy) == 1:
        return fuzzy[0].setname, "mapped"
    if len(fuzzy) > 1:
        return ",".join(identity.setname for identity in fuzzy), "ambiguous"
    return "", "unmapped"


def api_get(params: dict[str, str]) -> dict:
    query = urllib.parse.urlencode({"format": "json", **params})
    with urllib.request.urlopen(f"{API}?{query}", timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))


def category_files(limit: int | None) -> list[str]:
    titles: list[str] = []
    cont: dict[str, str] = {}
    while True:
        data = api_get(
            {
                "action": "query",
                "list": "categorymembers",
                "cmtitle": CATEGORY,
                "cmtype": "file",
                "cmlimit": "50",
                **cont,
            }
        )
        titles.extend(item["title"] for item in data["query"]["categorymembers"])
        if limit is not None and len(titles) >= limit:
            return titles[:limit]
        if "continue" not in data:
            return titles
        cont = {key: str(value) for key, value in data["continue"].items()}
        time.sleep(0.1)


def file_url(title: str) -> str:
    data = api_get(
        {
            "action": "query",
            "titles": title,
            "prop": "imageinfo",
            "iiprop": "url",
        }
    )
    pages = data["query"]["pages"]
    page = next(iter(pages.values()))
    return page["imageinfo"][0]["url"]


def download(url: str, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url, timeout=60) as response:
        path.write_bytes(response.read())


def run(args: argparse.Namespace) -> int:
    identities = load_identities(args.mame_db)
    originals = args.out / "originals"
    mapped = args.out / "mapped"
    report = args.out / "report.tsv"
    originals.mkdir(parents=True, exist_ok=True)
    mapped.mkdir(parents=True, exist_ok=True)

    rows = ["status\tsetname\ttitle\toriginal\tmapped"]
    for title in category_files(args.limit):
        name = title.removeprefix("File:")
        setname, status = map_filename(name, identities)
        original = originals / name.replace("/", "_")
        mapped_path = ""
        if not args.no_download:
            download(file_url(title), original)
        if status == "mapped":
            suffix = original.suffix or ".png"
            mapped_file = mapped / f"{setname}{suffix.lower()}"
            if not args.no_download:
                mapped_file.write_bytes(original.read_bytes())
            mapped_path = str(mapped_file)
        rows.append(f"{status}\t{setname}\t{title}\t{original}\t{mapped_path}")

    report.write_text("\n".join(rows) + "\n")
    print(f"wikipedia_neogeo_import report={report}")
    return 0


def self_test() -> int:
    identities = [
        Identity("mslug3", "Metal Slug 3 (NGM-2560)", title_keys("Metal Slug 3 (NGM-2560)")),
        Identity(
            "kof98",
            "The King of Fighters '98: The Slugfest / King of Fighters '98: dream match never ends",
            title_keys("The King of Fighters '98: The Slugfest / King of Fighters '98: dream match never ends"),
        ),
        Identity("aof2", "Art of Fighting 2", title_keys("Art of Fighting 2")),
    ]
    cases = {
        "NEOGEO Metal Slug 3.png": ("mslug3", "mapped"),
        "NEOGEO The King of Fighters '98 - The Slugfest.png": ("kof98", "mapped"),
        "NEOGEO Art of Fighting 2 screenshot.jpg": ("aof2", "mapped"),
    }
    for filename, expected in cases.items():
        actual = map_filename(filename, identities)
        if actual != expected:
            raise AssertionError(f"{filename}: expected {expected}, got {actual}")
    print("wikipedia_neogeo_import self-test ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mame-db", type=Path, default=Path("build/mame.sqlite3"))
    parser.add_argument("--out", type=Path, default=Path("build/neogeo-screenshots/wikipedia"))
    parser.add_argument("--limit", type=int)
    parser.add_argument("--no-download", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.mame_db.exists():
        parser.error(f"MAME DB not found: {args.mame_db}")
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
