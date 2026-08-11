# MiSTer frontend build policy

For local macOS UI design and deterministic RGB565 captures, see
[UI_PREVIEW.md](UI_PREVIEW.md).

Agents build and validate through `scripts/agent`; they do not invoke Cargo,
Apple container, cross, or deployment implementation commands directly.

The typed build state machine is:

```text
Infer → Preflight → PrepareContainer → Compile → Verify → Receipt → Complete
```

`BuildSpec` infers profile, UI scope, production features, toolchain, artifact,
and cache identity from changed components and delivery intent. Apple Silicon
uses Apple `container`; Linux CI may use the explicitly typed cross adapter.
There is no automatic backend fallback.

Every artifact receipt records the exact clean source SHA, lock/toolchain/cache
identity, target, profile, features, and output digest. Runtime and platform
delivery reject mismatched or dirty receipts.

Use:

```text
$magik-rust-lsp
git add -- PATH...
git commit -m "Describe the change"
git push
scripts/agent deliver
```

The analyzer provides bounded Rust diagnostics while editing. Pre-commit checks
cheap commit safety; pre-push and CI own builds, tests, feature matrices, and
platform assurance.
`deliver` owns build scope, artifact qualification, transport, activation,
rollback, and smoke verification. Human-only fixed scene operation is available
through `scripts/agent device scene`; it is separate from building and deployment.

## Local FPGA signoff

Apple Silicon can run the complete matched FPGA signoff locally:

```text
QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup
scripts/agent fpga signoff
```

Setup installs pinned Quartus Lite 17.0 Build 595 into the ignored local cache.
Quartus itself runs in an amd64 Apple container under Rosetta; the official
installer uses a QEMU amd64 chroot because it cannot complete under Rosetta.
Signoff reads the local `main` ref without switching the caller's worktree,
uses isolated generated source checkouts, builds the same stock, pinned
pre-observer, and final seed-1 variants as CI, and runs the same delta checker.
Rosetta requires Quartus parallel synthesis to be disabled; fitter and timing
remain parallel. That compatibility setting is applied identically to all
three variants and is part of the synthesis-cache identity.

Completed synthesis is cached per variant before the checker runs. A failing
signoff is therefore reproducible without another synthesis pass. Stock,
pre-observer, and final inputs have independent keys, so observer RTL changes
rebuild only the final variant and workflow or documentation-only commits do
not trigger synthesis. New results are built in staging directories and
promoted only after completion, preserving a valid cache across cancellation
or failure. Use an absolute `MISTER_FPGA_LOCAL_ROOT` shared by worktrees when
another local agent must reuse the install and completed variants. Local RBFs
remain diagnostic artifacts; only the GitHub platform workflow can publish or
qualify an RBF for deployment.

Compilation intent is explicit:

| Intent | Cargo policy | Artifact use |
| --- | --- | --- |
| Unit tests, Clippy, checks, pre-push, host CI | Dev/test, optimization 0, no debug info or LTO, incremental, 256 codegen units | Correctness only |
| Ordinary PR and main ARM CI | `ci-fast`, with the same compile-first settings | Linked non-production diagnostics |
| Runnable labs and captures | `release-live` or the owning device/profile release profile | Performance-sensitive iteration and evidence |
| Delivery | `release-device` runtime plus optimized manager | Installed development runtime |
| Alpha, beta, and release publication | `release-device` runtime plus optimized manager | Published production artifact |

`ci-fast` must never feed delivery, packaging, binary-size evidence, or a
release channel. Conversely, ordinary CI must not pay for release optimization;
its linked artifacts exist to prove the complete ARM application and agent
still link and satisfy their shared-library contracts.

## Compile-policy benchmark

The controlled 2026-08-07 revision comparison measured the parent revision
`7fac463ff3dfae69c124543f161b4cef8c0e24f7` against the completed compile-policy
implementation at `b678bf0ed3bfda461d6aacee9379f66e0745520d`. Both revisions used
the same Apple Silicon Mac, Rust 1.97.1, isolated target directories, and fresh
assurance evidence state for every sample.

For the catalog pre-push path, the five source-edit rebuilds changed from
40.049, 43.588, 46.978, 44.956, and 43.725 seconds to 20.956, 20.958,
21.231, 21.749, and 20.771 seconds. The median fell from 43.725 seconds to
20.958 seconds, a 2.09x speedup. The fully cold measurement improved from
71.836 seconds to 63.594 seconds, or 1.13x; dependency compilation and
non-compiler assurance overhead dominate that slice, so it is reported
separately rather than presented as a compiler-only result.

The authoritative samples, source hashes, commands, profiles, machine details,
and toolchain versions are in
`history/toolchain-bench/compile-policy-pre-push-20260807.json`. Reproduce the
comparison with clean absolute baseline and candidate repositories and new
paths outside both repositories:

```text
scripts/agent compile-time compare-revisions \
  --baseline-repository /absolute/clean/baseline \
  --candidate-repository /absolute/clean/candidate \
  --work-root /absolute/new/work-root \
  --output /absolute/new/report.json \
  --scenario pre-push-catalog
```

An ARM comparison can be selected with `--scenario arm-runtime-ci`; it builds
the legacy PR release profile, legacy main `release-device`, and candidate
`ci-fast` through typed Apple-container build specifications without contacting
a MiSTer. The 2026-08-07 ARM sampling run was stopped before it completed, so
no ARM speedup ratio is claimed in the recorded evidence.

The canonical `release-device` runtime includes dormant on-device profiling
support and retains function symbols. Benchmarks activate that support only on
the already-installed runtime. `scripts/agent build runtime-analysis` produces
the `release-device-profile` offline artifact and is not callable from the
benchmark workflow or deployable by it. Runtime delivery always
publishes `mister-magik-fb` together with its regenerated
`platform-v3.manifest`; no binary-only build or deployment command is exposed.
