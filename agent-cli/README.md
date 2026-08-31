# agent-cli

`agent-cli` is the repository workflow engine behind `scripts/agent`. It owns
exact-SHA builds, transactional delivery, benchmarks, diagnosis, and attended
release qualification. Python owns pre-push and CI host assurance.

The AI-facing commands are:

```text
scripts/agent plan
scripts/agent deliver
scripts/agent deliver game-databases --game-databases-release-dir PATH
scripts/agent restart-ui
scripts/agent benchmark
scripts/agent benchmark particles
scripts/agent benchmark catalog-lifecycle
scripts/agent benchmark launch-return
scripts/agent diagnose
scripts/agent clean
```

`restart-ui` sends delivery's acknowledged `mister_magik_suspend` followed by
`mister_magik_resume`. It does not build, stage, transfer, or replace files, and
it does not reboot Linux. The resume acknowledgement waits for the fresh launcher
child to become active.

The normal AI loop is edit with bounded Rust analyzer diagnostics where
applicable, stage intentional paths with ordinary Git, commit, and push.
`scripts/agent plan` previews the bootstrap-free Python pre-push checks and CI
ownership without executing them. The Git pre-commit hook runs a bootstrap-free
Python gate against the index under a fail-closed ten-second deadline. The
pre-push hook runs the Python gate and affected Python tests directly; CI owns
Cargo, ARM, visual, and full Python assurance.
Workflow evidence analysis uses the hidden typed
`scripts/agent db report` command rather than direct SQL.

`clean` discovers every tracked or untracked, non-ignored `Cargo.toml` in the
repository and runs `cargo clean --manifest-path` for each project. This keeps
new first-party projects covered without traversing ignored vendor or build
trees.

`scripts/agent release qualify` is an attended operator command. Hidden typed
build and host-assurance intents exist for CI and release tooling, not as a
public flag matrix. Commit
creation belongs to Git; `agent-cli` never stages, commits, or pushes.
`deliver` uses the exact clean local app commit for the app and manager. Main,
the scanout kernel plugin, and the latch RBF come only from the latest
published GitHub platform release. The tag-addressed cache is reused when it
still verifies against the latest release.

The `deliver game-databases` target accepts only an already-verified local
release directory and invokes only the database transaction. It does not
resolve platform releases or build, replace, or restart any platform/runtime
artifact.

`benchmark` profiles the already-installed development app in place. With no
scenario it runs the screensaver benchmark. A positional scenario selects
another registered typed workflow. `particles` searches and confirms the
60 FPS ceiling of both particle presets at 960x540, captures representative
frames, and restores the original display configuration. `catalog-lifecycle`
performs an isolated full catalog build under `/tmp`, validates every generated
shard, then removes the fixture and restores the ordinary launcher. Benchmarks
run through a restricted typed client that rejects delivery requests. They
never build or deploy a temporary runtime, and require the installed revision
to match the clean local commit wherever runtime or platform files changed.
Host-only benchmark changes may reconcile as a no-op without replacing the
installed runtime.

The launcher builds and runs `agent-cli` with a compile-first development
profile: no optimization or debug information, incremental compilation, and
wide codegen parallelism. Set `MISTER_AGENT_CLI_PROFILE=release` only when an
optimized workflow-engine binary is itself required. Cargo profile environment
overrides remain available when full host debug symbols are explicitly needed;
manifest, target-directory, and binary overrides remain available for tests and
specialized host environments.

Compile-policy changes are measured with the hidden, build-only
`scripts/agent compile-time compare-revisions` interface. It requires separate
clean baseline and candidate worktrees, an external new work root, an explicit
scenario, and a new JSON output path. The comparison never installs, deploys,
or runs an ARM artifact.

Device operations use `DeviceClient` and closed `DeviceRequest` variants.
The separate Rust `mister` binary remains available to humans for fixed operator
operations, but `agent-cli` never invokes it as a subprocess. Explicitly
read-only requests receive one bounded retry after transient unavailability;
mutations are not blindly replayed. Diagnosis may clear reboot arming and issue
one raw Linux reboot over SSH when the coherent installed platform has a missing
or stalled launcher. It refuses known reboot-instability state and never
automatically issues a second reboot.
