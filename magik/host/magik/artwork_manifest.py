"""Pure manifest validation shared by release packaging and Dev delivery."""

import re

SOURCE_BYTES = 360 * 504 * 3
MAX_INDEX_BYTES = 128 * 1024
MAX_FILES = 120
ROOT_KEYS = {
    f"root:{name}"
    for name in (
        "arcade",
        "consoles",
        "computers",
        "handhelds",
        "favourites",
        "settings",
    )
}


def files_for(index):
    if not isinstance(index, dict) or (
        index.get("schema"),
        index.get("width"),
        index.get("height"),
        index.get("format"),
    ) != (1, 360, 504, "RGB888"):
        raise ValueError("unsupported card artwork index")
    cards = index.get("cards")
    if not isinstance(cards, dict) or not ROOT_KEYS <= cards.keys():
        raise ValueError("card artwork index is missing root cards")
    files = {}

    def add(entry, extension, size):
        if not isinstance(entry, dict):
            raise ValueError("invalid card artwork entry")
        name, digest = entry.get("file"), entry.get("sha256")
        if (
            not isinstance(name, str)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*\." + extension, name)
            or not isinstance(digest, str)
            or not re.fullmatch(r"[0-9a-f]{64}", digest)
            or type(size) is not int
            or not 16 < size <= 2 * 1024 * 1024
        ):
            raise ValueError("invalid card artwork source")
        value = (digest, size)
        if name in files and files[name] != value:
            raise ValueError("conflicting card artwork declarations")
        files[name] = value

    for source in cards.values():
        add(source, "rgb888", SOURCE_BYTES)
        if not isinstance(source.get("contains_name", False), bool):
            raise ValueError("invalid card artwork name policy")
        prepared = source.get("prepared")
        if prepared is not None:
            if not isinstance(prepared, dict):
                raise ValueError("invalid prepared card artwork")
            add(prepared, "cardtex", prepared.get("bytes"))
    if len(files) > MAX_FILES:
        raise ValueError("card artwork file count exceeds limit")
    return files
