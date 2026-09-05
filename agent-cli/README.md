# agent-cli

`agent-cli` retains platform delivery, specialized qualification, diagnosis,
and attended device operations. Python owns pre-push and CI host assurance.

Ordinary application development uses the shared [2.0 tooling](../magik2/README.md):

```sh
scripts/magik2 deploy
scripts/magik2 check
scripts/magik2 watch
scripts/magik2 check idle
scripts/magik2 check idle --profile
```

The real app is the default; use `--app mini-magik` for the fast experiment.
`check` runs one smoke journey. Measurement and profiling are explicit scenarios.

Retained legacy operations require an explicit purpose:

```text
scripts/agent plan
scripts/agent deliver platform
scripts/agent deliver local-main
scripts/agent deliver game-databases --game-databases-release-dir PATH
scripts/agent benchmark input-integrity
scripts/agent diagnose
```

Bare `deliver`, `deliver runtime`, `restart-ui`, and bare `benchmark` are removed.
There are no forwarding aliases. `deliver platform` names the existing platform
transaction; reconciliation can still select a runtime-only update or no-op.
It is not the everyday app-development path.

Validation ownership is defined in root `AGENTS.md`. Use `scripts/agent plan`
for the fast-check preview and `scripts/agent guidance PATH` for source ownership.
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
`deliver platform` uses the exact clean local app commit for the app and manager. Main,
the scanout kernel plugin, and the latch RBF come only from the latest
published GitHub platform release. The tag-addressed cache is reused when it
still verifies against the latest release.

The `deliver game-databases` target accepts only an already-verified local
release directory and invokes only the database transaction. It does not
resolve platform releases or build, replace, or restart any platform/runtime
artifact.

`benchmark input-integrity` is the sole retained benchmark. It verifies Main's
controller proxy and kernel input path; application scenarios use 2.0. The
historical application benchmark registry and `alpha accept` UI acceptance
command are removed. No replacement command claims their former release coverage.
The separate release/Main-return and FPGA workflows remain.

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
