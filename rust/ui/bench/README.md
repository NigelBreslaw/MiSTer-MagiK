# Slint bench scenes (toolchain / device)

Base canvas **960×540** at `ui-scale=1`; all layout lengths are `base * ui-scale`.
Set scale from Rust via `MISTER_RENDER_SCALE` (1 or 2) before show.

```bash
MISTER_RENDER_SCALE=1 /media/fat/mister-magic/mister-magic-fb ui list_scroll 15   # 960×540
MISTER_RENDER_SCALE=2 /media/fat/mister-magic/mister-magic-fb ui list_scroll 15   # 1920×1080
```

Bench: `.venv/bin/python scripts/bench_list_scroll.py`

Shared window: [`../mister_window.slint`](../mister_window.slint) (`in-out property ui-scale`).

Scenes: `demo`, `full_motion`, `static_ui`, `local_motion`, `text_heavy`, `solid_fill`, `list_scroll`.
