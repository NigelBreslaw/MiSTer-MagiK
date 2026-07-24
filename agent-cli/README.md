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
scripts/agent diagnose
```

The normal AI loop is edit, `check` as needed, stage intentional paths with
ordinary Git, then commit. Argument-free validation uses all working-tree
changes. The Git pre-commit hook runs `verify --staged` against exactly the
index. Workflow evidence analysis uses the hidden typed
`scripts/agent db report` command rather than direct SQL.

`scripts/agent release qualify` is an attended operator command. Hidden typed
build intents exist for CI compatibility, not as a public flag matrix. Commit
creation belongs to Git; `agent-cli` never stages, commits, or pushes.
`deliver` uses the exact clean local app commit and clean local `Main_MiSTer`
`mister-magik` commit as its source authority, whether or not either commit is
published.

The launcher builds and runs `agent-cli` with Cargo's release profile. Explicit
manifest, target-directory, and binary overrides remain available for tests and
specialized host environments.

Device operations use `DeviceClient` and closed `DeviceRequest` variants.
The separate Rust `mister` binary remains available to humans for fixed operator
operations, but `agent-cli` never invokes it as a subprocess.
