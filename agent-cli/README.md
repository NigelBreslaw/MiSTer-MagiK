# MiSTer MagiK Agent CLI

`agent-cli` is the typed workflow harness for humans and AI agents working in
this repository. It chooses the smallest relevant checks, captures noisy child
output, records evidence, and reports concise progress. It does not provide a
generic remote shell and does not replace `scripts/mister` as the device safety
boundary.

Run from the repository root:

```bash
cargo run --quiet --manifest-path agent-cli/Cargo.toml -- lint --working-tree
cargo run --quiet --manifest-path agent-cli/Cargo.toml -- plan lint --paths agent-cli
cargo run --quiet --manifest-path agent-cli/Cargo.toml -- --output ndjson lint --staged
cargo run --quiet --manifest-path agent-cli/Cargo.toml -- arm check-launcher
cargo run --quiet --manifest-path agent-cli/Cargo.toml -- scripts review
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

## Audit evidence

The database is `<git-common-dir>/agent-cli/agent.sqlite3`, shared by every
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
the host release gate currently execute their established script
implementations behind typed operations. They should be deleted only after a
deeper Rust implementation passes compatibility tests; their shell branches
must not be transliterated into Rust merely to reduce the script count.

The deletion evidence and undecided human-workflow candidates are recorded in
[`docs/agents/script-deletion-ledger.md`](../docs/agents/script-deletion-ledger.md).

