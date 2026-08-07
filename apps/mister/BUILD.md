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

The canonical `release-device` runtime includes dormant on-device profiling
support and retains function symbols. Benchmarks activate that support only on
the already-installed runtime. `scripts/agent build runtime-analysis` produces
the `release-device-profile` offline artifact and is not callable from the
benchmark workflow or deployable by it. Runtime delivery always
publishes `mister-magik-fb` together with its regenerated
`platform-v3.manifest`; no binary-only build or deployment command is exposed.
