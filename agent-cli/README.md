# agent-cli

`agent-cli` is the repository workflow engine behind `scripts/agent`. It owns
Git-aware affected validation, exact-SHA builds, transactional delivery,
benchmarks, diagnosis, and attended release qualification.

The AI-facing commands are:

```text
scripts/agent plan
scripts/agent check
scripts/agent verify
scripts/agent deliver
scripts/agent benchmark
scripts/agent benchmark catalog-lifecycle
scripts/agent diagnose
```

The normal AI loop is edit, `check` as needed, stage intentional paths with
ordinary Git, then commit. Argument-free validation uses all working-tree
changes. The Git pre-commit hook runs the fail-closed, ten-second
`pre-commit` fast gate against the index. The pre-push hook runs full affected
verification for the exact branch commit, while CI remains authoritative.
Workflow evidence analysis uses the hidden typed
`scripts/agent db report` command rather than direct SQL.

`scripts/agent release qualify` is an attended operator command. Hidden typed
build intents exist for CI compatibility, not as a public flag matrix. Commit
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
clean local commit.

The launcher builds and runs `agent-cli` with Cargo's release profile. Explicit
manifest, target-directory, and binary overrides remain available for tests and
specialized host environments.

Device operations use `DeviceClient` and closed `DeviceRequest` variants.
The separate Rust `mister` binary remains available to humans for fixed operator
operations, but `agent-cli` never invokes it as a subprocess.
