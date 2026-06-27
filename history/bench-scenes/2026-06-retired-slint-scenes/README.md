Retired Slint Bench Scenes
==========================

These files are snapshots of the earliest MiSTer MagiK Slint benchmark scenes.
They were useful for renderer, dirty-rect, and toolchain experiments before the
launcher and media paths became the production benchmark surface.

Historical measurements remain in `history/toolchain-bench/results.tsv`.
New performance evidence should use launcher scenarios, media/video profiling,
or experiment-specific scripts under `scripts/experiments/`.

Archived scenes:

- `demo.slint` - the original generic Slint demo window.
- `full_motion.slint` - broad animation/dirty-region stress scene.
- `static_ui.slint` - idle/static repaint probe.
- `local_motion.slint` - small moving-region probe.
