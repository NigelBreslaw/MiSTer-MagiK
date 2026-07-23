# agent-cli

`agent-cli` is the repository workflow engine behind `scripts/agent`. It owns
task baselines, affected validation, exact-SHA builds, commits, transactional
delivery, benchmarks, diagnosis, and attended release qualification.

The AI-facing commands are:

```text
scripts/agent task begin
scripts/agent plan
scripts/agent check
scripts/agent verify
scripts/agent commit -m "Message"
scripts/agent deliver
scripts/agent benchmark
scripts/agent diagnose
```

`scripts/agent release qualify` is an attended operator command. Hidden typed
build intents exist for CI compatibility, not as a public flag matrix. `commit`
is the only lifecycle command that changes Git state;
`deliver` never commits or pushes.

The task identity supplied by Codex or `--task-id` is a persistent session
identity. Each successful `task begin` creates a new internal lifecycle for
that session, so follow-up commits in the same conversation do not replace
earlier baselines or delivery evidence. Begin every new edit batch before
editing. `task begin --replace` is recovery-only and refuses to replace an
active baseline after its files have changed.

The launcher builds and runs `agent-cli` with Cargo's release profile. Explicit
manifest, target-directory, and binary overrides remain available for tests and
specialized host environments.

Device operations use `DeviceClient` and closed `DeviceRequest` variants.
The separate Rust `mister` binary remains available to humans for fixed operator
operations, but `agent-cli` never invokes it as a subprocess.
