# Startup-particle consolidation baseline

Measured on 2026-08-02 before extracting the production Magik engine from the
full Slint application. The canonical build-only harness used a new target
directory, one cold build, five unchanged invocations, one source rebuild
warm-up, and five measured edits to `particle_engine.rs`. It restored the
source's exact bytes and timestamps and never contacted a MiSTer.

| Target | Cold | No-op median | Particle rebuild median |
| --- | ---: | ---: | ---: |
| macOS full UI preview | 79.267 s | 3.171 s | 3.092 s |

The adjacent JSON report is authoritative. The ARM build-only target was also
validated through the repository's Apple-container workflow. Its five-sample
measurement remains to be recorded before the extraction commit if a directly
comparable ARM timing is required.

Command:

```text
scripts/agent compile-time measure magik-full-app-macos \
  --target-dir /private/tmp/mister-magik-full-app-before-macos-20260802 \
  --output /private/tmp/mister-magik-full-app-before-macos-20260802.json
```

Environment: Apple Silicon `arm64`, macOS 26.5.2, Rust 1.97.1, Cargo 1.97.1.
