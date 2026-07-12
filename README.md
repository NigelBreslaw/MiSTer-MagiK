# MiSTer MagiK

MiSTer MagiK is a frontend for the MiSTer FPGA focused on making it easy to
manage your game collection, get set up, and start playing with ease.

It is built to feel polished, playful, and commercial-quality, with a whimsical
interface that makes browsing retro games part of the fun.

To make MiSTer usable, MiSTer MagiK is **highly opinionated**.

If playing with `.ini` files, getting lost in display modes, tuning scaler
settings, and never quite having a working joystick is your idea of fun, you
probably will not like this frontend.

But if you love 90s arcade effects, smooth 60fps transitions, fast game
discovery, and the feeling of stumbling into the next brilliant retro game to
play, you might just love it.

MiSTer MagiK wants MiSTer to feel less like a configuration project and more
like a magic cabinet.

## Important Disclaimer

MiSTer MagiK changes system configuration and controls low-level aspects of a
MiSTer installation. Installing and using it is at your own risk. Back up your
SD card and read the full [MiSTer MagiK disclaimer](disclaimer.md) before use.

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

MiSTer MagiK is licensed under the terms in this repository's `LICENSE` file.

MiSTer MagiK also includes or builds on open source software, fonts, tools, and
libraries from the broader Rust, Slint, and MiSTer ecosystems. Their respective
licenses remain with their authors.
