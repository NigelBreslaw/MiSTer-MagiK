# MiSTer MagiK tooling

`scripts/magik` builds, deploys, observes and tests MiSTer MagiK. Mini-MagiK uses
that same service and harness for small experiments and benchmarks.

Install Python 3.12+, `uv`, and Apple's `container` with its service running.
The Python test driver uses the existing private Slint package index.
Select a device once with `scripts/magik device select ADDRESS`; discovery then
follows its identity across address changes and worktrees. SSH bootstrap credentials
live in macOS Keychain; native tokens stay in the permission-restricted user store.

```sh
scripts/magik deploy
scripts/magik check
scripts/magik watch
scripts/magik device status
scripts/magik mcp
scripts/magik bench blend
scripts/magik bench blend --visual
scripts/magik bench blend --counters neon
```

Real application delivery uses the development installation and Main-owned launch.
It does not replace the production application. Use `--app mini-magik` with
`deploy`, `check` or `watch` for Mini. Compatible services are reused by capability;
a build identifier mismatch does not require installation. Missing capabilities
are supplied automatically through the existing bootstrap/update path.

Source builds always invoke Cargo in the checkout's build container. Cargo owns
incremental freshness, including native C and header inputs; the host reports
whether the selected binary was fresh or rebuilt. Existing build-cache JSON
files are no longer used. Prebuilt overrides still bypass compilation, and an
unchanged installed artifact still transfers zero bytes. Warm source builds
include container/Cargo startup overhead.

`check` runs one smoke journey. Request idle/motion checks or profiles explicitly.
Benchmarks use two timing repetitions; visual and PMU passes are separate commands.
Python scenarios are the place for focused UI and hardware assertions. Hardware
claims still require the relevant device observation; synthetic input alone does
not establish physical controller or CRT behavior.

- [Device discovery and authentication](docs/device-discovery.md)
- [Device, catalog and platform operations](docs/native-operations.md)
- [Framebuffer images in Codex](docs/framebuffer-capture.md)
- [Benchmarks and profiling](docs/benchmarks.md)
- [Desktop's direct connection](docs/desktop-api.md)
- [Build storage](docs/build-storage.md)

Source ownership is `host/magik` for Python, `agent` for the Rust service, and
`probe` for Mini-MagiK. Shared application instrumentation lives in
`crates/tooling-support`; real-app instrumented builds enable its `tooling` feature.

Installed storage paths and application environment keys retain their existing
identifiers so a tooling rename does not lose credentials or invalidate a running
application. The public command, source directory and Python package are `magik`.

Run focused checks through `scripts/cargo` and the affected Python tests. GitHub
runs ordinary CI without a special label or a required tooling/consumer PR split.
