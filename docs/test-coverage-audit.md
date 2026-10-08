# MiSTer MagiK test coverage audit

The 8 October 2026 audit covers the committed project at `d4e38a168` in the isolated `nigel/coverage-audit` worktree. The original checkout's existing edits are preserved. Coverage includes all 24 first-party Rust manifests and supplementary Python host and script fixtures. The highest-value remaining tests are production event-loop journeys, service transactions, and hardware-adapter lifecycle sequences.

Seven behavior defects were reproduced or established from the dependency graph and fixed. Twelve new regression tests cover controller migration, idle-clock boundaries, global INI insertion, upload digests, wire framing, catalog integrity, and descriptor reservation. Existing failures and full-feature lint failures were also repaired.

## Coverage measurements

Measurements use `cargo-llvm-cov 0.8.7`, Rust 1.99.0, and the macOS AArch64 host. Final coverage is regenerated from clean audit build artifacts with normal cleanup enabled. The tool removes measured-package artifacts and raw profiles to avoid stale mappings; see its [cleanup implementation](https://github.com/taiki-e/cargo-llvm-cov/blob/v0.8.7/src/clean.rs).

These are LLVM line and region percentages for the measured configurations. They include inline tests and build scripts. They are not a production-only percentage, branch coverage, or a combined project percentage. Different feature configurations have different denominators, and dependency code can be exercised by more than one consumer.

| Manifest directory | Lines | Regions |
| --- | ---: | ---: |
| `apps/desktop` | 56.42% | 56.63% |
| `apps/framebuffer-scene-lab` | 61.21% | 58.87% |
| `apps/mister` | 66.47% | 67.22% |
| `apps/mister/ui-generated` | 71.43% | 80.49% |
| `crates/catalog` | 85.53% | 84.91% |
| `crates/controller-registry` | 90.19% | 92.50% |
| `crates/framebuffer-scenes` | 85.18% | 86.32% |
| `crates/framebuffer-stream` | 94.84% | 92.22% |
| `crates/magik-core` | 91.22% | 93.01% |
| `crates/media-contract` | 90.39% | 90.77% |
| `crates/mister-ini` | 99.75% | 99.29% |
| `crates/particles` | 85.88% | 86.70% |
| `crates/perf-events` | 78.47% | 79.25% |
| `crates/screenshot-parade` | 92.03% | 92.71% |
| `crates/tooling-support` | 88.84% | 86.41% |
| `crates/visual-concepts` | 93.74% | 94.64% |
| `magik/agent` | 50.55% | 49.90% |
| `magik/probe` | 37.84% | 33.20% |
| `mister/platform/contracts/latch` | 89.27% | 90.43% |
| `mister/platform/contracts/manifest` | 89.49% | 91.93% |
| `mister/platform/contracts/scanout` | 87.18% | 88.64% |
| `mister/platform/runtime` | 72.95% | 74.21% |
| `mister/tools/manager` | 78.37% | 79.56% |
| `tools/usb-video` | 30.20% | 30.04% |

The device app is measured with all features and `MISTER_UI_BUILD_SCOPE=all`; its default and standalone `asset-tools` configurations are also measured. The desktop companion is measured in both live and compiled UI modes. Runtime and scene lab use all features. Catalog uses `builder,development-layout,io-test-metrics`, controller registry uses `io-probe`, media contract uses signed manifests, and framebuffer scenes enable launcher profiling. The UI-generated package percentage covers its build script, not generated Slint bindings.

The tracked inventory contains 396 Rust files, with LLVM mappings for 357 in the measured configurations. Files without a mapping are listed separately rather than counted as zero percent. `uncovered-functions.csv` identifies 1773 functions with no observed calls and at least five mapped source lines, excluding recognizable test modules and integration-test files. This is a test-selection aid, not proof that code is unused or a measure of assertion quality.

Python coverage uses coverage.py 7.16.2 with branch measurement. The combined fixture run passed 644 tests and 92 subtests.

| Python area | Statements covered | Branches covered |
| --- | ---: | ---: |
| Host orchestration | 63.21% | 55.78% |
| CI and release orchestration | 69.51% | 55.64% |
| Repository check scripts | 30.92% | 23.90% |
| Tracked Blender studio scripts | 0.00% | 0.00% |

Python subprocesses are not automatically instrumented by this run. The host fixture suite also passed independently with 310 tests. None of these percentages establishes coverage of device-only Linux/ARM paths, NEON C, kernel providers, FPGA HDL, Bash, JavaScript, CSS, or Slint source. The source inventory records those boundaries. Vendor, private submodules, and read-only reference repositories are separate projects.

## Proposed tests

Prioritize observable behavior through existing production interfaces. Each fixture should compare data, pixels, protocol bytes, or committed files rather than assert only that a helper was called.

| Priority | Area and measured gap | Concrete test | Required assertions |
| --- | --- | --- | --- |
| 1 | Device app event loop and startup; `launcher_loop.rs` and `app_entry.rs` have large uncovered bodies | Replay captured-style input batches, display-status observations, catalog replies, and full Slint redraws through the real loop/session interfaces | One clock advance per produced frame; captured press/release order; same-frame Arcade repaint after a full Slint present; correct idle reuse and drop classification |
| 1 | Service media, telemetry, publication, and catalog dispatch | Use local TCP/Unix sockets and `Agent::with_state_root` fixtures for authenticated requests, interrupted uploads, failed publication, replay, and malformed stream frames | Stable wire responses; no unwanted file replacement; temporary-file and descriptor cleanup; a failed old request cannot commit a new generation |
| 1 | Runtime full-frame and hidden presenters have little host execution coverage | Drive the existing transport/planner boundaries with pending, active, unknown, timeout, and failed-post sequences | Never write an active/pending slot; both slots recover correct cached damage and direct layers; ambiguous posts require the existing recovery state |
| 1 | Desktop `main.rs` and native discovery lifecycle | Run the application controllers against a local fake native service and a temporary SD tree, including reconnect, cancellation, stale responses, and truncated frame updates | Actual models update only for the current connection/request; stale frames cannot overwrite current state; cached pixels and file lists remain correct |
| 2 | Mini-MagiK startup, controller save, incremental refresh, and retained tiles | Exercise startup and save acknowledgements with a temporary state directory; render successive damage generations through the actual retained-buffer consumer | Save generations commit in order; failures preserve accepted state; incremental pixels equal a full-render reference |
| 2 | USB-video native implementation is not entered by current fixtures | Separate sample-buffer conversion and session lifecycle tests from an attended macOS capture journey | Respect row stride and orientation; reject malformed dimensions; cancellation releases callbacks and session resources; real camera permission/device failures remain explicit |
| 2 | Catalog preview and refresh error paths remain partially covered | Corrupt or remove adjacent artifacts, change generation/fingerprint bindings, fail a publication step, and cancel preview work after a newer selection | Previous valid catalog remains readable; readers reject incorrect bindings; late previews never replace the current selection; failure leaves no accepted partial state |
| 2 | Particle and scene CRT routes, reload failures, and source-specific morph paths | Render NTSC/PAL routes and transition boundaries from real recipe fixtures; reload malformed and replaced recipe files | Deterministic RGB565 pixels; exact storyboard boundaries; valid old recipe survives failure; no retained-buffer residue across route or generation changes |
| 2 | Screenshot preparation and stream subscribers under failure | Combine generation replacement, exhausted buffer reservoirs, reader disconnects, blocked readiness, and worker failure | Buffers return exactly once; stale generations are discarded; bounded queues preserve the latest accepted frame; consumed pixels equal the serial reference |
| 2 | Manager transactional file and Main-selection failure paths | Build temporary installed-layout fixtures with missing or invalid components and failed replacement steps | Typed INI edits converge without losing unrelated content; failed transactions retain prior installed components and selection |
| 3 | Media update policy and manifest consumer decisions | Run the real consumer workflow for off, check-only, download, invalid policy, missing index, and stale pack/index states | Only the required artifact is requested; check-only leaves files intact; malformed signed envelopes cannot advance saved state |
| 3 | Python child-process entrypoints, CLI failures, and Blender scripts | Instrument subprocesses explicitly; run CLI fixtures on disposable roots and Blender automation against a temporary scene | Stable exit status and output; path and ownership checks hold; generated scene structure and material settings meet the existing verification contracts |

Linux/ARM and FPGA cases should use the existing typed CI or attended device harnesses. Host fixtures can establish planner and protocol behavior; scanout timing, physical input, Main handoff, PMU capture, and real device recovery need their corresponding hardware evidence.

## Verification and artifacts

Raw JSON, exact command records, failures before fixes, and final logs are retained under ignored `build/coverage-audit/`. The package and file CSVs are portable paths relative to the repository:

- `coverage-packages.csv`: one primary measured configuration for each manifest.
- `coverage-files.csv`: per-file statistics for primary and additional measured configurations.
- `uncovered-functions.csv`: non-trivial functions with no observed execution in the selected reports.
- `source-inventory.csv`: all tracked first-party source files and their measurement status.
- `python-coverage.json`: statements and branches by Python file.
- `fresh-runs.json`: exact clean final coverage commands and exit codes.

The changed Rust manifests receive focused test and Clippy checks with warnings denied, including full frontend/runtime feature builds. Formatter and diff checks cover the final source. The actual launcher software renderer was captured with `ui,asset-tools`: title and footer glyphs render correctly. The standalone generator produced 19 resources identical to the committed resources using disposable inputs and outputs.

Rust LSP routed to the isolated worktree and reported no errors for catalog and service files. Its frame-clock request was cancelled, and its current feature configuration did not link the runtime UI module. Source references, Cargo compilation, regression tests, and Clippy provide validation for those edits; analyzer roots/settings were not broadened.

A typical measurement command, run from the worktree, is:

```sh
CARGO_TARGET_DIR=build/coverage-audit/target scripts/cargo llvm-cov \
  --locked --manifest-path crates/catalog/Cargo.toml \
  --features builder,development-layout,io-test-metrics \
  --tests --include-build-script --json \
  --output-path build/coverage-audit/catalog.json
```

For the full frontend:

```sh
MISTER_UI_BUILD_SCOPE=all CARGO_TARGET_DIR=build/coverage-audit/target \
  scripts/cargo llvm-cov --locked --manifest-path apps/mister/Cargo.toml \
  --all-features --tests --include-build-script --json \
  --output-path build/coverage-audit/mister.json
```

Standalone `tooling-support` and `ui-generated` have no committed lockfile. Their consumer lockfiles were copied temporarily for the audit and removed from the working changes. The profiler fix uses `scripts/magik-ci dependencies sync apps/mister/Cargo.toml`; its lockfile change only removes the duplicate package identity and updates the two corresponding dependency references.

## Bugs fixed

| Defect | Reproduction or evidence | Result |
| --- | --- | --- |
| Completed controller setup could inherit a pending entry's USB port during v1 migration | A deterministic regression exercises both merge orders; the old code returns a mixed entry | Completed setup retains its confirmed port and entry; a real v1 file migrates and saves the current schema |
| Idle clock accounting loops once per elapsed period and caps a call at `u32::MAX` periods | Five seconds with a one-nanosecond period times out before the fix; duration-limit cases check saturation | Whole periods are accounted for with constant-time arithmetic and the sub-period remainder is retained |
| Inserting an unsectioned INI key creates an unwanted `[]` header | Empty, global-only, and named-section fixtures fail before the fix | Global keys are inserted before named sections, preserving comments and newline behavior |
| Valid uppercase SHA-256 uploads are rejected, and accepted hashes can retain inconsistent case | Uppercase upload regression fails with `sha256 mismatch`; real transfer fixture checks the returned hash | Hex verification accepts either case, rejects wrong digests, and protocol replies use canonical lowercase |
| `asset-tools` supplies empty font resources to runtime and test builds | Combined UI/generator flags cause 78 rendering/font failures; standalone generator tests also fail | UI and tests load real generated fonts for every feature combination; standalone generation keeps bootstrap placeholders |
| Profile and tooling features link two identities of the same `pprof` commit | The lockfile contains branch and revision identities; the linker reports duplicate `perf_signal_handler` symbols | The frontend uses the same pinned revision as shared tooling; one package and signal handler remain |
| Catalog protocol descriptors can occupy standard I/O slots | An isolated exec helper closes stdin and the old duplicator returns descriptor 0 | Protocol descriptors start above stderr and retain close-on-exec; the leakage test checks the original socket rather than a reused descriptor number |

Existing tests also had incorrect feature/layout expectations: catalog defaults under development layout, the removed `flip.project` stage, the old `Option` latch-planning API, missing hardware-completion transitions in three ownership fixtures, overlapping scroll-stripe expectations, full invalidation mistaken for a partial restore, reversed clockwise preview pixels, and the shell descriptor-number assumption. Those cases now assert the current contracts; rotation covers both directions with complete golden buffers.

Full-feature strict lint failures were resolved with behavior-preserving corrections: error constructors, let-chain guards, checked division with the same zero fallback, slice copying/chunks, private error variant names, immutable timer arguments, newline formatting, helper placement, and asset-example subset reuse. Narrow lint exceptions preserve allocation-free publication recovery and the benchmark's explicit measured phase fields.
