# Desktop scanout visibility — 2026-07-11

## Confirmed cause

The desktop showed launcher and framebuffer-stream health but had no indication
whether the device was using atomic scanout, waiting for it, or running the
legacy fallback. That made a healthy producer stream easy to misread as proof of
the physical HDMI scanout path.

## Before / after

- Before: zero scanout facts in Analytics and zero scanout rows in Debug.
- After: Analytics has one compact status strip above framebuffer controls;
  Debug Runtime has one scanout row showing state plus mode/module/device facts.
- Desktop-only display work does not change device performance. Production
  work-p99 remains Home 6,888 us, Arcade 3,736 us, preview 2,469 us pending the
  device AFTER benchmarks.

## Tests and visual verification

- `cargo test --manifest-path desktop/Cargo.toml` — 97 passed.
- `cargo check --manifest-path desktop/Cargo.toml --no-default-features --features compiled-ui`
- `cargo check --manifest-path desktop/Cargo.toml --features slint/mcp,live-ui,skia-renderer`
- `desktop/scripts/mcp-smoke.sh`
- Slint MCP tree: one 1100x760 logical window, Debug Runtime tab and Analytics
  Framebuffer page both reachable.
- Slint MCP screenshots visually verified without clipping or overlap:
  `/private/tmp/mister-magik-runtime-scanout.png` and
  `/private/tmp/mister-magik-analytics-scanout.png`.

## Evidence artifacts

- `desktop/ui/views/analytics.slint`
- `desktop/ui/views/debug.slint`
- `desktop/src/app_state.rs`
- `desktop/src/agent_client.rs`
- `history/2026-07-11-scanout-observability.md`
