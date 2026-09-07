# Host utilities

Device operations use `scripts/magik`; platform operations use `scripts/magik-platform`.
See [native operations](../magik/docs/native-operations.md).

Host-only operations use existing Python tooling:

- `scripts/magik-ci dependencies sync PATH/Cargo.toml` resolves only the owning
  manifest and adjacent lockfile. `--package NAME` requests a targeted update.
- `scripts/magik-ci clean --manifest PATH/Cargo.toml --package NAME` cleans that
  package. Global cache cleaning is retired.
- `scripts/magik-ci compile-time measure TARGET --kind cold|incremental
  --target-dir build/CACHE --output REPORT.json` records exactly two repetitions.
  Cold measurements require a new cache; incremental measurements require an
  existing prepared cache. No implicit warmup, edits, campaign or baseline run.
- `scripts/magik-ci compile-time compare BASELINE.json CANDIDATE.json` compares
  compatible successful two-sample files. ARM measurements reuse the shared Apple
  container/FFmpeg preparation; host measurements use the normal Cargo wrapper.
- `scripts/magik-ci capture-usb [--seconds N] [--output STEM]` invokes the standalone
  `tools/usb-video` AVFoundation executable. It has no device-agent dependency.
- `scripts/magik-ci evidence export --database FILE --output FILE` exports old
  workflow evidence through a read-only SQLite connection. No new database.
- `scripts/magik-ci guidance PATH` and `plan` provide bootstrap-free host guidance
  and validation previews.
- `scripts/magik-platform fpga setup --local-root DIR` prepares the existing
  Apple Quartus installation. `fpga signoff --stock REPORT --baseline REPORT
  --patched REPORT` runs the existing offline timing/CDC comparison. FPGA synthesis
  remains an explicitly requested GitHub platform workflow, only when requested.
