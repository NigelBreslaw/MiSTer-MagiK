# agent-cli

`agent-cli` is the repository workflow engine behind `scripts/agent`. It owns
Git-aware affected validation, exact-SHA builds, transactional delivery,
benchmarks, diagnosis, and attended release qualification.

The AI-facing commands are:

```text
scripts/agent plan
scripts/agent deliver
scripts/agent benchmark
scripts/agent benchmark catalog-lifecycle
scripts/agent diagnose
```

The normal AI loop is edit with bounded Rust analyzer diagnostics where
applicable, stage intentional paths with ordinary Git, commit, and push.
`scripts/agent plan` previews the full affected assurance plan without executing
it. The Git pre-commit hook runs the fail-closed, ten-second fast gate against
the index. The pre-push hook runs full affected assurance for the exact branch
commit, while CI remains authoritative.
Workflow evidence analysis uses the hidden typed
`scripts/agent db report` command rather than direct SQL.

`scripts/agent release qualify` is an attended operator command. Hidden typed
build and host-assurance intents exist for CI and release tooling, not as a
public flag matrix. Commit
creation belongs to Git; `agent-cli` never stages, commits, or pushes.
`deliver` uses the exact clean local app commit for the app and manager. Main,
the scanout kernel plugin, and the latch RBF come only from the latest
published GitHub platform release. The tag-addressed cache is reused when it
still verifies against the latest release.

`benchmark` profiles the already-installed development app in place. With no
scenario it runs the screensaver benchmark. A positional scenario selects
another registered typed workflow; `catalog-lifecycle` performs an isolated
full catalog build under `/tmp`, validates every generated shard, then removes
the fixture and restores the ordinary launcher. Benchmarks never build or
deploy a temporary runtime, and require the installed revision to match the
clean local commit wherever runtime or platform files changed. Host-only
benchmark changes may reconcile as a no-op without replacing the installed
runtime.

The launcher builds and runs `agent-cli` with Cargo's release profile. Explicit
manifest, target-directory, and binary overrides remain available for tests and
specialized host environments.

Device operations use `DeviceClient` and closed `DeviceRequest` variants.
The separate Rust `mister` binary remains available to humans for fixed operator
operations, but `agent-cli` never invokes it as a subprocess.
