# MiSTer MagiK

MiSTer MagiK is a frontend for the MiSTer FPGA.

It is built to feel polished, playful, and commercial-quality, with a whimsical
interface that makes browsing retro games part of the fun. MagiK is built to
have insane levels of performance. It is built to be fast, smooth, and responsive.

It aims to help you find and enjoy retro games with ease and find surprises in
your game collection.

To make MiSTer usable, MiSTer MagiK is **highly opinionated**.

MiSTer MagiK wants MiSTer to feel less like a configuration project and more
like a magic cabinet. If low level configuration is your jam, MagiK might not be
for you. However if it is for you and you still like tweaking the hell out of
ini files the settings menu has an 'Exit to MiSTer' option to put you instantly
back into the original OSD menu system.

## Important Disclaimer

MiSTer MagiK changes system configuration and controls low-level aspects of a
MiSTer installation. Installing and using it is at your own risk. Back up your
SD card and read the full [MiSTer MagiK disclaimer](disclaimer.md) before use.

## Beta installation and updates

MiSTer MagiK Beta is installed through the MiSTer Downloader used by
`update_all`. [Download the Beta installer ZIP](https://github.com/NigelBreslaw/MiSTer-MagiK/releases/download/beta/mister-magik-beta-installer.zip),
extract it to the SD-card root, run `update_all`, then run `Scripts` ->
`MiSTer-MagiK` once and reboot normally.
Later updates require only `update_all` and a normal reboot. If you run the
script again it will offer the option to uninstall MiSTer MagiK.
Only the MagiK launcher belongs in the Scripts menu. See
[installer safety and upgrade notes](docs/installer.md) for the obsolete
constants helper and failed-package recovery.

## Application development

Use `scripts/magik2 deploy`, `scripts/magik2 check`, and `scripts/magik2 watch`.
These target the real development app and `/media/fat/mister-magik-dev` data.
Use `--app mini-magik` for the fast experiment. The default check is one smoke
journey; benchmarks and profiles are explicit. See [development setup and
commands](magik2/README.md). Production installation above is a separate workflow.

See [retired experiment tooling and remaining legacy consumers](docs/tooling-retirement.md)
for the deletion milestone.

## Built With Slint

MiSTer MagiK is built with [Slint](https://slint.dev), a modern declarative UI
toolkit for Rust, C++, JavaScript, and embedded systems.

Slint is a huge part of what makes MiSTer MagiK possible. It gives the project a
real UI language, a clean Rust integration model, and a path to building rich
animated interfaces on hardware that was never designed to run a modern desktop
environment.

MiSTer MagiK uses Slint as the foundation for its launcher experience: smooth
transitions, responsive controls, structured components, and a UI that can keep
growing without turning into a pile of one-off framebuffer code.

If you are building embedded UI, hardware UI, kiosk software, or anything that
needs to feel polished without dragging in a full desktop stack, Slint is worth
a serious look.

## Licenses

MiSTer MagiK first-party source is Copyright (C) 2026 Nigel Breslaw and is
licensed under GPL-3.0-or-later. Active source and configuration files carry
machine-readable SPDX headers. This includes the first-party Linux kernel
module. The module source license is GPL-3.0-or-later; its Linux loader
classification is a compatibility marker and is recorded separately in its
metadata.

MiSTer MagiK also includes or builds on open source software, fonts, tools, and
libraries from the broader Rust, Slint, and MiSTer ecosystems. Their respective
licenses remain with their authors.
