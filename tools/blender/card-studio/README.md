<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# MiSTer MagiK card studio

This directory contains the editable Blender source for the six borderless 5:7
launcher card artworks. The cards contain models and lighting only so the app can
draw its border, number, title, and tagline dynamically.

The project targets Blender 5.2 LTS and uses Cycles. Its dark satin housings and
illuminated glass trims are procedural. The `.blend` contains no inspiration
images, external image textures, fonts, logos, linked libraries, or pre-rendered
card images.

## Scenes

- `MAGIK_01_ARCADE`
- `MAGIK_02_CONSOLES`
- `MAGIK_03_HANDHELDS`
- `MAGIK_04_COMPUTERS`
- `MAGIK_05_SETTINGS`
- `MAGIK_06_FAVOURITES`

Each scene is framed for a 1500 by 2100 pixel output. The lower part of the image
is deliberately quiet so the app can place dynamic text there.

## Render

Run a fast 600 by 840 preview of every card:

```sh
blender -b MiSTer-MagiK-Card-Studio.blend -P render_cards.py -- --quality preview
```

Render final 1500 by 2100 images, or select one or more scene suffixes:

```sh
blender -b MiSTer-MagiK-Card-Studio.blend -P render_cards.py -- --quality final
blender -b MiSTer-MagiK-Card-Studio.blend -P render_cards.py -- --quality preview --scene 03_HANDHELDS
```

Renders are written below the ignored `renders/` directory by default. Pass
`--output /path/to/folder` to choose another destination.

## Maintain and verify

`apply_satin_material.py` rebuilds the metric dual-scale molded satin shader on
the six main housings without changing model, camera, or lighting transforms:

```sh
blender -b MiSTer-MagiK-Card-Studio.blend -P apply_satin_material.py
```

Run the project integrity checks after an edit:

```sh
blender -b MiSTer-MagiK-Card-Studio.blend -P verify_project.py
```

The verifier rejects file-backed or packed images, font objects, linked
libraries, missing scenes, non-Cycles renderers, and material recipe drift.
