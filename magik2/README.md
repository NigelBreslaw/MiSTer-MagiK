# MiSTer MagiK Tooling 2.0

One development workflow for **Mini-MagiK**, the fast Slint experiment in
`probe/`, and the real **MiSTer MagiK** launcher. Both share native delivery,
observation, device-clock measurement windows, CPU profiles and Python tests.
The real application runs as a separate development copy under
`/media/fat/mister-magik2/magik`; installed production app/platform files are
not replaced.

## Setup and commands

Install Python 3.12+, `uv`, and Apple's `container` with its service running.
Configure `MISTER_IP`; SSH bootstrap uses `MISTER_USER` and `MISTER_PASS`.
The existing `slint-testing==0.3` private-index authentication must be available
to uv. Keep credentials in the environment/user credential store, never Git.

```sh
scripts/magik2 deploy
scripts/magik2 check smoke
scripts/magik2 check motion
scripts/magik2 check motion --profile
scripts/magik2 watch
scripts/magik2 status
scripts/magik2 stop

# Same workflow, real application development copy:
scripts/magik2 deploy --app magik
scripts/magik2 check smoke --app magik
scripts/magik2 check idle --profile --app magik
scripts/magik2 watch --app magik
```

`deploy` automatically prepares a checkout-specific Apple container and compiles
only stale artifacts. The checked-in Containerfile pins Rust 1.98.0 and the ARM
userspace toolchain. Cold image/dependency preparation takes longer; warm targets
apply after preparation. `scripts/magik2 build agent` and `build --app mini-magik` expose
preparation independently. Cargo registry/git caches persist under
`~/.cache/mister-magik2/cargo` (`MISTER_MAGIK2_BUILD_CACHE` overrides this).
Each checkout has its own mount, target artifacts, and validated build cache.

Device tokens are shared across worktrees under
`$XDG_STATE_HOME/mister-magik2` or `~/.local/state/mister-magik2`.
`MISTER_MAGIK2_STATE` explicitly overrides that location. A new branch retrieves
an existing token and retains a compatible service. Only missing operation
capabilities cause a native upgrade. SSH installs/repairs a service only when
native support is absent/unavailable. For explicit repair testing,
`MISTER_MAGIK2_REPAIR=1` uses native replacement whenever reachable.

An unchanged ready artifact transfers zero bytes. A published hash is distinct
from the running hash; start checks its expected hash before changing processes.
The agent owns the selected app after disconnect and restores it after test sessions.
`stop` confirms Main's acknowledgement and observed launcher readiness.
No matching-version, clean-commit, platform qualification, or rollback gate is
part of this development path.

## One scenario system

The consumer scenarios live in `scenarios/` and run through pytest:

```sh
uv run --project magik2/host pytest magik2/scenarios --collect-only
uv run --project magik2/host pytest magik2/scenarios --magik2-device -k motion
```

Without `--magik2-device` they skip hardware operations. Mini-MagiK motion has two separate
unprofiled repetitions, each two seconds warm-up and five measured seconds on
the device clock. `--magik2-profile` selects only one ten-second measured
profile case; it does not also run the ordinary benchmark cases. The CLI `check` commands invoke these same tests. Profiling is not
included in benchmark aggregates. Correctness/evidence failures fail the check;
performance numbers never block a later deployment. The real-app scenario is
explicitly **idle**, measuring the ordinary launcher loop, not a synthetic FPS
comparison with Mini-MagiK. Its two windows and separate optional profile use
the same fixture and application-side measurement code. Ordinary smoke and
measurement cases share a single app session, with one restoration when the
command finishes. Repetitions reset the workload without restarting the app. Results retain each
sample and min/median/max; two runs are not a percentile estimate.

The same binary contains testing/profiling support. Profiles have unique run IDs,
completion and sample-count evidence, folded stacks and a flamegraph. Counters
separate render time, render-to-present time, latch rejections and physical drops.

## Observation and results

`watch` serves only localhost. Both apps use one latest-preview slot and a
background sender; slow consumers drop previews without blocking rendering.
The shared publisher uses raw RGB565 keyframes at at most 5 Hz. That simple codec
choice trades bandwidth for less device work; observation overhead is measured.
The native agent bounds transfer memory, time, and active connection count.

`build/magik2-results/<run>/` contains finalized `run.json`, `events.jsonl`, real
`logs.txt`, device diagnostics, and relevant screenshot/profile files. Records
include artifact/input hashes, agent identity, all primary/cleanup failures,
and timings. Credentials are not included. Result retention is local, with no
workflow database. See [acceptance](ACCEPTANCE.md) for the indexed attempts.

## Focused verification and ownership

```sh
uv run --project magik2/host pytest magik2/host/tests -q
scripts/cargo test --manifest-path magik2/agent/Cargo.toml --locked
scripts/cargo test --manifest-path magik2/probe/Cargo.toml --locked -p mister-magik-tooling-support
```

Shared application support lives in `crates/tooling-support`. The real app's
`magik2` feature enables it with Slint testing; ordinary production builds do
not enable the integration. Real previews come from the existing committed
latch buffer, including direct composition. Measurements use existing render
boundaries and validated physical presentation telemetry. Idle repeated vblanks
are retained as evidence, not claimed to be missed animation deadlines.

The two app definitions in `host/magik2/apps.py` select package, binary and build
profile. There is no app manifest or matching-version requirement. A missing
`applications` or `measurement` capability upgrades the service automatically.
The real app reuses its existing `release-device-ui-tests` profile, Cortex-A9
flags and minimal FFmpeg 8.1.2 recipe. Its existing private font submodule is
initialized when absent; private assets are never copied into this repository.

Core changes use a dedicated tooling PR and `magik2-tooling` label. Probe and
scenario edits are consumer work. CI checks scope and the focused tests; it has
no device access and does not impose runtime delivery gates.

Run `scripts/magik2 acceptance` for two attempts each of unchanged deploy,
changed prebuilt deploy, Rust edit and Slint edit. It temporarily edits the
probe and restores its original sources and app in cleanup; use a checkout
that no other process is editing during the run. `scripts/magik2 acceptance
--contracts` checks rejected uploads, superseded starts, viewer streams,
viewer-on motion, disconnected test attachment and observed Main recovery.
The timing matrix reports both attempts and the slower time, without percentile estimates.
Both retain indexed results. See [review corrections](docs/corrections.md).


The bounded old/new transfer comparison is recorded in
[the transfer report](docs/milestone-2-transfer-check.md). Use sustained saved
MB/s for app-size comparisons; Mini-MagiK compile times are not full-app targets.

Current implementation and the remaining hardware checks are recorded in
[the milestone evidence](docs/milestone-2.md).
