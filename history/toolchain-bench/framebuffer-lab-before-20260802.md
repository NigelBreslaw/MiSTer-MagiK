# Framebuffer lab compile-time baseline

Measured on 2026-08-02 before extracting the experimental particle showcase
from the full Slint application. Both measurements used new target directories,
one cold build, five unchanged invocations, one particle rebuild warm-up, and
five measured particle rebuilds. The corrected v2 harness appended a unique
generation comment to `showcase.rs` for every forced sample, then restored its
exact original bytes and timestamps. Its SHA-256 was identical before and after
each run.

| Target | Cold | No-op median | Particle rebuild median |
| --- | ---: | ---: | ---: |
| macOS full UI preview | 75.328 s | 2.817 s | 2.680 s |
| ARM full `release-live` app | 139.863 s | 26.056 s | 26.722 s |

The adjacent `*-v2-20260802.json` files are authoritative. The original v1 raw
reports are retained but superseded because their future-mtime technique did
not prove that Cargo rebuilt every forced sample. The ARM no-op result is
intentionally reported: the current build metadata causes the full application
crate to compile and link on an unchanged invocation.

Commands:

```text
scripts/agent compile-time measure baseline-macos \
  --target-dir /private/tmp/mister-magik-compile-before-macos-v2-20260802 \
  --output /private/tmp/mister-magik-compile-before-macos-v2-20260802.json

scripts/agent compile-time measure baseline-arm \
  --target-dir /private/tmp/mister-magik-compile-before-arm-v2-20260802 \
  --output /private/tmp/mister-magik-compile-before-arm-v2-20260802.json
```

Environment: Apple Silicon `arm64`, macOS 26.5.2, Rust 1.97.1, Cargo 1.97.1.
The ARM route used the repository's Apple-container state machine and existing
minimal FFmpeg cache. Neither route launched an executable or contacted a
MiSTer.
