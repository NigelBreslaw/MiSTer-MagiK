"""Attribute cold-build CPU samples and blocked time to functions and paths."""

import bisect
import collections
import re
import struct
import subprocess
from pathlib import Path

# 32-bit ARM EABI numbers for the syscalls that block on the SD card.
SYSCALLS = {
    "3": "read",
    "4": "write",
    "38": "rename",
    "39": "mkdir",
    "118": "fsync",
    "180": "pread64",
    "181": "pwrite64",
    "217": "getdents64",
    "322": "openat",
    "327": "fstatat64",
    "362": "syncfs",
    "397": "statx",
}


def load_segments(elf: bytes) -> list[tuple[int, int, int]]:
    """`(file offset, virtual address, size)` of each PT_LOAD of a 32-bit ELF."""
    if elf[:4] != b"\x7fELF" or elf[4] != 1:
        raise ValueError("expected a 32-bit little-endian ELF executable")
    (program_headers,) = struct.unpack_from("<I", elf, 0x1C)
    entry_size, count = struct.unpack_from("<HH", elf, 0x2A)
    segments = []
    for index in range(count):
        kind, offset, address, _, size, *_ = struct.unpack_from(
            "<8I", elf, program_headers + index * entry_size
        )
        if kind == 1:
            segments.append((offset, address, size))
    return segments


def parse_nm(text: str) -> list[tuple[int, str]]:
    """Sorted `(address, name)` of defined function symbols from `nm -n`."""
    symbols = []
    for line in text.splitlines():
        parts = line.split(" ", 2)
        if len(parts) == 3 and parts[1] in {"t", "T", "w", "W"}:
            # Thumb function symbols carry the instruction-set bit.
            symbols.append((int(parts[0], 16) & ~1, parts[2]))
    return sorted(symbols)


def parse_maps(text: str) -> list[tuple[int, int, int, str]]:
    """`(start, end, file offset, path)` of each executable mapping."""
    mappings = []
    for line in text.splitlines():
        fields = line.split(maxsplit=5)
        if len(fields) < 5 or "x" not in fields[1]:
            continue
        start, end = (int(part, 16) for part in fields[0].split("-"))
        mappings.append(
            (start, end, int(fields[2], 16), fields[5] if len(fields) > 5 else "")
        )
    return mappings


class Symboliser:
    def __init__(self, executable, maps, segments, symbols):
        self.executable = executable.removesuffix(" (deleted)")
        self.maps = maps
        self.segments = segments
        self.symbols = symbols
        self.addresses = [address for address, _ in symbols]

    @classmethod
    def from_files(cls, executable: str, maps: str, elf_path: Path):
        symbols = subprocess.run(
            ["nm", "-C", "--defined-only", "-n", str(elf_path)],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        return cls(
            executable,
            parse_maps(maps),
            load_segments(elf_path.read_bytes()),
            parse_nm(symbols),
        )

    def name(self, address: int) -> str:
        for start, end, offset, path in self.maps:
            if start <= address < end:
                if path.removesuffix(" (deleted)") != self.executable:
                    return f"[{Path(path).name or 'anon'}]"
                file_offset = address - start + offset
                virtual = next(
                    (
                        base + file_offset - segment
                        for segment, base, size in self.segments
                        if segment <= file_offset < segment + size
                    ),
                    file_offset,
                )
                index = bisect.bisect_right(self.addresses, virtual) - 1
                if index < 0:
                    return f"exe+{virtual:x}"
                return re.sub(r"::h[0-9a-f]{16}$", "", self.symbols[index][1])
        return "[unmapped]"


def fold_stacks(stacks: str, thread_names: dict[str, str], symboliser: Symboliser):
    """Folded on-CPU stacks, outermost frame first, one count per sample."""
    folded: collections.Counter[str] = collections.Counter()
    for line in stacks.split("\n")[1:]:
        if not line:
            continue
        tid, pc, lr, frames = (line.split("\t") + [""])[:4]
        thread = thread_names.get(tid, tid)
        if thread == "build-evidence":
            continue
        # A return address is the instruction after the call.
        callers = [
            symboliser.name(int(frame, 16) - 1) for frame in frames.split(",") if frame
        ]
        stack = [symboliser.name(int(pc, 16))]
        link = symboliser.name(int(lr, 16) - 1)
        if not callers or callers[0] != link:
            stack.append(link)
        stack += callers
        stack = [
            name
            for index, name in enumerate(stack)
            if index == 0 or name != stack[index - 1]
        ]
        folded[";".join([thread] + stack[::-1])] += 1
    return folded


def rank_functions(folded: collections.Counter[str], top: int):
    """Self and inclusive sample counts, as `(percent, name)` lists."""
    total = sum(folded.values()) or 1
    leaf: collections.Counter[str] = collections.Counter()
    inclusive: collections.Counter[str] = collections.Counter()
    for stack, count in folded.items():
        frames = stack.split(";")[1:]
        if frames:
            leaf[frames[-1]] += count
        for name in set(frames):
            inclusive[name] += count

    def percentages(counter):
        return [
            (round(100 * count / total, 1), name)
            for name, count in counter.most_common(top)
        ]

    return {
        "samples": total,
        "self": percentages(leaf),
        "inclusive": percentages(inclusive),
    }


def blocked_by_path(tasks: str, interval_ms: int, top: int, depth: int = 4):
    """Seconds spent in uninterruptible I/O waits, by syscall and directory."""
    seconds: collections.Counter[tuple[str, str]] = collections.Counter()
    for line in tasks.split("\n")[1:]:
        fields = line.split("\t")
        if len(fields) < 7 or fields[3] != "D" or fields[2] == "build-evidence":
            continue
        parts = fields[6].split("/")
        # Paths are absolute, so the first element is empty.
        target = "/".join(parts[: depth + 1]) if fields[6] else "?"
        seconds[(SYSCALLS.get(fields[5], fields[5]), target)] += interval_ms
    return [
        {"syscall": syscall, "path": path, "seconds": round(ms / 1000, 2)}
        for (syscall, path), ms in seconds.most_common(top)
    ]
