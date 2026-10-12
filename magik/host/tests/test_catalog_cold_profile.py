import struct

import pytest
from magik import catalog_cold_profile as profile

MAPS = (
    "00010000-00030000 r-xp 00000000 08:01 1 /media/fat/mister-magik-fb\n"
    "00030000-00040000 rw-p 00020000 08:01 1 /media/fat/mister-magik-fb\n"
    "b6e00000-b6f00000 r-xp 00000000 08:01 2 /lib/libc-2.31.so\n"
)
NM = (
    "00011000 T app::scan\n"
    "00011101 t app::walk\n"
    "00012000 T app::publish::h0123456789abcdef\n"
    "00013000 b app::data\n"
)


def elf_with_one_load(offset, address, size):
    header = bytearray(0x34)
    header[:5] = b"\x7fELF\x01"
    struct.pack_into("<I", header, 0x1C, 0x34)
    struct.pack_into("<HH", header, 0x2A, 32, 1)
    return bytes(header) + struct.pack("<8I", 1, offset, address, 0, size, size, 5, 0)


def symboliser():
    return profile.Symboliser(
        "/media/fat/mister-magik-fb",
        profile.parse_maps(MAPS),
        [(0, 0x10000, 0x20000)],
        profile.parse_nm(NM),
    )


def test_load_segments_rejects_non_arm_executables():
    assert profile.load_segments(elf_with_one_load(0, 0x10000, 0x20000)) == [
        (0, 0x10000, 0x20000)
    ]
    with pytest.raises(ValueError, match="32-bit"):
        profile.load_segments(b"MZ" + bytes(60))


def test_parse_nm_keeps_functions_and_clears_the_thumb_bit():
    assert profile.parse_nm(NM)[:2] == [
        (0x11000, "app::scan"),
        (0x11100, "app::walk"),
    ]
    assert all("data" not in name for _, name in profile.parse_nm(NM))


def test_symboliser_names_executable_libraries_and_unmapped_addresses():
    names = symboliser()
    assert names.name(0x11020) == "app::scan"
    assert names.name(0x12004) == "app::publish"
    assert names.name(0xB6E10000) == "[libc-2.31.so]"
    assert names.name(0x90000000) == "[unmapped]"


def test_fold_stacks_orders_outermost_first_and_skips_the_sampler():
    stacks = "tid\tpc\tlr\tframes\n7\t11110\t12005\t12005,11001\n9\t11010\t11010\t\n"
    folded = profile.fold_stacks(
        stacks, {"7": "worker", "9": "build-evidence"}, symboliser()
    )
    assert folded == {"worker;app::scan;app::publish;app::walk": 1}
    ranked = profile.rank_functions(folded, 3)
    assert ranked["samples"] == 1
    assert ranked["self"] == [(100.0, "app::walk")]
    assert (100.0, "app::publish") in ranked["inclusive"]


def test_blocked_time_groups_io_waits_by_syscall_and_directory():
    tasks = (
        "t_ms\ttid\tcomm\tstate\twchan\tsyscall\tdetail\n"
        "0\t1\tw\tD\t\t322\t/media/fat/games/GBA/Game One\n"
        "10\t1\tw\tD\t\t322\t/media/fat/games/GBA/Game Two\n"
        "20\t1\tw\tR\t\t\t\n"
        "30\t1\tw\tD\t\t4\t/media/fat/mister-magik-dev/state/file\n"
        "40\t2\tbuild-evidence\tD\t\t322\t/media/fat/games/GBA/x\n"
    )
    assert profile.blocked_by_path(tasks, 10, 5) == [
        {"syscall": "openat", "path": "/media/fat/games/GBA", "seconds": 0.02},
        {
            "syscall": "write",
            "path": "/media/fat/mister-magik-dev/state",
            "seconds": 0.01,
        },
    ]
