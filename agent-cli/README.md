# MiSTer MagiK Agent CLI

`agent-cli` is the typed workflow harness for humans and AI agents working in
this repository. It chooses the smallest relevant checks, captures noisy child
output, records evidence, and reports concise progress. It does not provide a
generic remote shell and does not replace `scripts/mister` as the device safety
boundary.

Run from the repository root:

```bash
scripts/agent plan --working-tree
scripts/agent check --paths agent-cli
scripts/agent --output ndjson verify --staged
scripts/agent arm check-launcher
scripts/agent scripts review
```

With no arguments in a terminal, the program opens its Ratatui operator view.
With no terminal, it retains concise human output. Automation should explicitly
select `--output ndjson`.

## Progress contract

NDJSON is versioned and contains one complete event per line. Events are
`started`, `progress`, `warning`, `completed`, or `failed`. A progress event
contains a run ID, sequence number, elapsed milliseconds, short phase and
message, and an optional integer percentage.

- Start, phase changes, warnings, failure, and completion emit immediately.
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
destructive. Apple-container builds are local-write operations and use the
canonical `apps/mister/build-arm.sh` implementation without automatic retry.

`scripts/mister` remains the sole device adapter. Deployment, reboot, fault
injection, mode switching, and release acceptance remain outside typed device
execution until their cleanup and recovery behavior is modeled as state
machines and tested with a fake transport. The CLI must never expose arbitrary
SSH or SFTP as an AI operation.

## Compatibility implementations

Repository validation, Rust development checks, doctor, host-tool checks, and
the host release gate are typed operations. Focused deep check scripts remain;
the retired orchestration entrypoints are not compatibility interfaces.

The deletion evidence and undecided human-workflow candidates are recorded in
[`docs/agents/script-deletion-ledger.md`](../docs/agents/script-deletion-ledger.md).
