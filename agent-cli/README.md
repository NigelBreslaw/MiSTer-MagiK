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

The normal AI loop is `task begin`, edit, `check` as needed, then `commit`.
`commit` stages task-owned paths and verifies that exact tree before creating
the commit. Do not run `verify` immediately before `commit`; standalone
`verify` exists for assurance without a commit. Workflow evidence analysis uses
the hidden typed `scripts/agent db report` command rather than direct SQL.

`scripts/agent release qualify` is an attended operator command. Hidden typed
build intents exist for CI compatibility, not as a public flag matrix. `commit`
is the only lifecycle command that changes Git state;
`deliver` never commits or pushes. It is task-independent: the exact clean
local app commit and clean local `Main_MiSTer` `mister-magik` commit are its
source authority, whether or not either commit is published.

The task identity supplied by Codex or `--task-id` is a persistent session
identity. Each successful `task begin` creates a new internal lifecycle for
that session, so follow-up commits in the same conversation do not replace
earlier baselines or delivery evidence. Begin every new edit batch before
editing. `task begin --replace` is recovery-only: it starts a new lifecycle
while carrying forward the active baseline, so existing task changes remain
visible and owned without a stash/restore cycle. Task lifecycles scope
validation and commits; they do not authorize delivery. When an abandoned
lifecycle from another session still claims paths, close it explicitly with
`scripts/agent --task-id ID task supersede`; this records abandonment without
attributing a commit to that task.

The launcher builds and runs `agent-cli` with Cargo's release profile. Explicit
manifest, target-directory, and binary overrides remain available for tests and
specialized host environments.

Device operations use `DeviceClient` and closed `DeviceRequest` variants.
The separate Rust `mister` binary remains available to humans for fixed operator
operations, but `agent-cli` never invokes it as a subprocess.
