# MiSTer MagiK Agent CLI

`agent-cli` is the typed workflow harness for humans and AI agents working in
this repository. It chooses the smallest relevant checks, captures noisy child
output, records evidence, and reports concise progress. It does not provide a
generic remote shell and does not replace `scripts/mister` as the device safety
boundary.

Run from the repository root:

```bash
scripts/agent task begin
scripts/agent plan
scripts/agent check
scripts/agent verify
scripts/agent commit -m "Describe the completed change"
scripts/agent --output ndjson verify --staged
scripts/agent scripts review
```

`task begin` snapshots the worktree under `CODEX_THREAD_ID` (or an explicit
`--task-id`). Normal planning and validation then use only files changed since
that baseline, including later edits to files that were already dirty. Explicit
`--paths` remains available for CI and diagnostics.

`commit -m MESSAGE` derives its file set from the active task baseline, refuses
ambiguous or pre-staged work, validates the staged result, and creates one
commit without pushing or bypassing hooks. It is an inherently Git-writing
operation and must receive `.git` write permission on its first invocation.
Validation can inspect later edits to baseline-dirty files, but commit refuses
those overlaps because it cannot safely separate task and pre-existing content.

With no arguments in a terminal, the program opens its Ratatui operator view.
With no terminal, it retains concise human output. Automation should explicitly
select `--output ndjson`.

## Progress contract

NDJSON is versioned and contains one complete event per line. Events are
`started`, `progress`, `warning`, `completed`, or `failed`. A progress event
contains a run ID, sequence number, elapsed milliseconds, short phase and
message, and an optional integer percentage.

- Successful operation names are retained as evidence but not printed.
- Warnings, external requirements, failure, and completion emit immediately.
- Active work emits one coalesced heartbeat after ten seconds of silence.
- Measurable work may emit at each new ten-percent boundary.
- Child stdout and stderr are never copied into NDJSON.
- A failed operation includes a short tail and the retained log path.

The Ratatui view consumes the same event model and redraws existing widgets
instead of appending terminal output.

## Cargo dependency policy

Dependency-consuming Cargo checks use the committed lockfile and local cache
first. If the cache is incomplete, the CLI retries the same locked operation
online once. A sandbox or unavailable network is reported as
`network_required` with the command to rerun with network access;
registry/download failures are `dependency_fetch_failed`, and only an executed
failing test is `test_failure`. Cargo formatting does not use this policy.

## Audit evidence

The database is `<primary-worktree>/.agent-cli/agent.sqlite3`, shared by every
linked worktree. WAL permits independent agents to record requests safely.
Requests are inserted before argument parsing, so malformed and policy-rejected
commands remain visible. Recognized credentials are redacted, and environment
variables are not captured unless explicitly allow-listed.

Request, plan, command, duration, outcome, and event metadata are retained.
Captured command logs may be removed with `agent-cli db prune-logs`; pruning
does not delete command metadata. The CLI fails closed if its request cannot be
durably recorded.

## Risk and device policy

Operations are classified as read-only, local-write, device-write, or
destructive. Required Apple-container checks may be planned automatically.
Quartus and RBF builds are prohibited locally on macOS; FPGA changes produce an
explicit GitHub Actions requirement instead.

`scripts/mister` remains the sole device adapter. Deployment, reboot, fault
injection, mode switching, and release acceptance remain outside typed device
execution until their cleanup and recovery behavior is modeled as state
machines and tested with a fake transport. The CLI must never expose arbitrary
SSH or SFTP as an AI operation.

## Validation ownership

The typed impact graph maps task paths to owned components, consumers, and the
minimum `check` or `verify` operations. Unclassified paths fail closed. Focused
deep-check scripts remain implementations, not AI-facing command choices.

The deletion evidence and undecided human-workflow candidates are recorded in
[`docs/agents/script-deletion-ledger.md`](../docs/agents/script-deletion-ledger.md).
