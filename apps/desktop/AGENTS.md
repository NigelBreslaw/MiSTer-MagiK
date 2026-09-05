# Desktop companion

UI starts at `ui/main.slint`, composition at `src/main.rs`, and SD browsing at
`src/sd_card.rs`. Primer and Material icons are vendor submodules.

Reusable Primer fixes belong in `vendor/github-app/packages/primer-slint`;
TreeViewRow changes must cover gallery literals. Load Material icons from disk
with `slint::Image::load_from_path` or the local helper, not generated tables.
Analytics proxies producer RGB565 streams; do not add host raw-to-PNG paths.

Use `apps/desktop/scripts/dev-live-mcp.sh` for live MCP and `mcp-smoke.sh` for
one endpoint check, both with first-attempt escalation. Do not improvise
environment-prefixed commands or alternate ports. `dev-live.sh` supplies live
Skia rendering; tests use the software renderer. Visually verify UI changes.
