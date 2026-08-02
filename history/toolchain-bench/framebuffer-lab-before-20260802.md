# Framebuffer lab compile-time baseline

Measured on 2026-08-02 before extracting the experimental particle showcase
from the full Slint application. Both measurements used new target directories,
one cold build, five unchanged invocations, one particle rebuild warm-up, and
five measured particle rebuilds. The harness temporarily advanced the mtime of
`showcase.rs`; its SHA-256 was identical before and after each run and its
original timestamps were restored.

| Target | Cold | No-op median | Particle rebuild median |
| --- | ---: | ---: | ---: |
| macOS full UI preview | 69.940 s | 2.581 s | 2.918 s |
| ARM full `release-live` app | 137.417 s | 26.625 s | 27.886 s |

The macOS warm-up and first particle sample encountered concurrent Cargo
package-cache locks; the complete samples are retained in the adjacent raw JSON
files rather than discarded. The ARM no-op result is intentionally reported:
the current build metadata causes the full application crate to compile and
link on an unchanged invocation.

Commands:

```text
scripts/agent compile-time measure baseline-macos \
  --target-dir /private/tmp/mister-magik-compile-before-macos-20260802 \
  --output /private/tmp/mister-magik-compile-before-macos-20260802.json

scripts/agent compile-time measure baseline-arm \
  --target-dir /private/tmp/mister-magik-compile-before-arm-20260802 \
  --output /private/tmp/mister-magik-compile-before-arm-20260802.json
```

Environment: Apple Silicon `arm64`, macOS 26.5.2, Rust 1.97.1, Cargo 1.97.1.
The ARM route used the repository's Apple-container state machine and existing
minimal FFmpeg cache. Neither route launched an executable or contacted a
MiSTer.
