<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Startup particle lab

This is the focused, Slint-free development app for the production-quality
MagiK text and arcade-cabinet particle effects. The recipe schema selects the
effect; there is deliberately no demo registry or recipe-family abstraction.

Run a live macOS preview with a validated recipe:

```text
mister-magik-startup-particle-lab --recipe path/to/magik.json
```

Saving the recipe reloads the renderer. The last valid renderer stays visible
after rejected or partial saves, and `status.json` is written beside the recipe.

Create a deterministic RGB565-derived PPM capture without opening a window:

```text
mister-magik-startup-particle-lab \
  --recipe path/to/cabinet.json \
  --time-ms 5000 \
  --output cabinet.ppm
```
