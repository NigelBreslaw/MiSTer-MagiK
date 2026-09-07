# Host tooling after legacy orchestration retirement

Device discovery, remembered authentication, ordinary app delivery, device
control, catalog/media operations and platform publication use 2.0. See
`magik2/docs/native-operations.md` for commands, activation semantics and recovery.
The thin `scripts/magik-platform` entrypoint delegates native work to the same
2.0 runtime; it does not build or invoke `agent-cli`.

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
  compatible successful two-sample files. ARM measurements reuse 2.0's Apple
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
  remains an explicitly requested GitHub platform workflow, never a migration test.

The combined `release qualify` gate and its certificate creators are removed.
This deliberately drops mandatory aggregate-board certification, six-hour stress
and automatic display/recovery matrices. Existing offline frame and aggregate
certificate readers remain usable for historical evidence. Physical input and
focused CRT qualification remain separately requested and are not replaced by
Python UI scenarios.

No Desktop migration or legacy service uninstall is included. The remaining
legacy consumer inventory is in `docs/tooling-retirement.md`.
