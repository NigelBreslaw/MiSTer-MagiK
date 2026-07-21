# AGENTS.md - desktop app

Desktop-specific notes for agents working in this folder. The root
`AGENTS.md` still applies, especially device safety and typed host-tool rules.
File authority is documented in `docs/agents/file-authority.md`.

## Current Shape

- This is the macOS desktop companion for MiSTer MagiK.
- The UI is Slint 1.17 and uses the vendored Primer package at:
  `apps/desktop/vendor/github-app/packages/primer-slint`.
- The main UI file is `ui/main.slint`.
- Rust owns data fetching, cache state, and Slint global updates in `src/main.rs`.
- SD Card browser state lives in `src/sd_card.rs`.
- The Analytics live toggle consumes the MagiK agent `framebuffer_stream_v1`
  path, which proxies producer-side RGB565 frames from `mister-magik-fb`.
  Manual capture/save still uses the agent framebuffer capture API for one-shot
  PNGs. Do not add raw `/dev/fb0` dump or host-side raw-to-PNG flows.
- Primer Slint and Material file icons are loaded from git submodules under
  `apps/desktop/vendor/`.

## Slint And MCP

- Prefer the live dev script while iterating:
  `apps/desktop/scripts/dev-live.sh`
- `apps/desktop/scripts/dev-live.sh` intentionally runs a release build because live
  reload behaves better there.
- Always visually verify UI changes with the Slint MCP server before calling
  them done. Query the live tree for structure and interactions; use screenshots
  when layout, color, or clipping needs actual visual inspection.
- The MCP endpoint is local, for example `http://127.0.0.1:9315/mcp`.
- Use the direct MCP wrapper for MCP sessions:
  `apps/desktop/scripts/dev-live-mcp.sh`. Do not start MCP by prefixing
  `MISTER_DESKTOP_MCP=1` or `SLINT_MCP_PORT=...` inline; stable wrapper commands
  make first-attempt escalation and approval matching work.
- Desktop app launches and local MCP probes are outside the workspace sandbox.
  Run both `apps/desktop/scripts/dev-live-mcp.sh` and `apps/desktop/scripts/mcp-smoke.sh`
  with `sandbox_permissions: "require_escalated"` on the first tool call. Start
  the app once, then verify the endpoint once with `apps/desktop/scripts/mcp-smoke.sh`;
  if the port is unavailable, report that result instead of retrying with a
  different command shape.
- For compiled UI validation use:
  `scripts/agent verify --paths apps/desktop`
- Desktop tests and coverage intentionally use Slint's software renderer by
  default so they do not trigger `skia-bindings` network fetches in cold or
  sandboxed environments. Use `apps/desktop/scripts/dev-live.sh` for the live Skia
  app path instead of adding `skia-renderer` to test/coverage commands.

## Primer Dependency

- Primer Slint is a git submodule:
  `apps/desktop/vendor/github-app`.
- After cloning this repo, initialize Primer with:
  `git submodule update --init apps/desktop/vendor/github-app`.
- If a desktop change exposes a reusable Primer component gap, fix it in
  `apps/desktop/vendor/github-app/packages/primer-slint`.
- `TreeViewRow` is an explicit Slint struct. If adding a field, update every
  struct literal in the Primer gallery too, not just the desktop app.

## Material Icon Theme

- The Material icons are a git submodule, not copied files:
  `apps/desktop/vendor/material-icon-theme`.
- After cloning this repo, initialize icons with:
  `git submodule update --init apps/desktop/vendor/material-icon-theme`
- Icons should be loaded from disk at runtime with `slint::Image::load_from_path`
  or the local `file_icons` helper. Do not use large `@image-url(...)` tables for
  the icon theme; that bakes assets into generated code and bloats binaries.
- The desktop app can override the icon directory with
  `MISTER_MAGIK_DESKTOP_MATERIAL_ICON_DIR`.
- Keep file rows lightweight: store an icon key in state and resolve it to a
  cached image at the UI boundary.

## Validation

Run the focused checks before committing desktop UI work:

```bash
scripts/agent plan --paths apps/desktop
scripts/agent check --paths apps/desktop
scripts/agent verify --paths apps/desktop
```

If you change the vendored Primer package, also run the relevant check there,
usually:

```bash
pnpm --filter slint-gallery run typecheck
```
