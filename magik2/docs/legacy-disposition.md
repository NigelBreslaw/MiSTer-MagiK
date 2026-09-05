# Legacy tooling: deletion decisions

Later implementation status is recorded in [Tooling retirement](../../docs/tooling-retirement.md).

This is a source-backed consumer inventory for milestone 3, not a parity backlog.
Do not migrate a command merely because it exists. Owners below identify code
boundaries, not commitments to implement replacement features.

| Current consumer / source | Decision | Boundary before deletion |
|---|---|---|
| Everyday app delivery and build orchestration (`agent-cli/src/cli.rs`, `agent-cli/src/host/mod.rs`) | Replace with the existing 2.0 commands; remove the old app-development path after documentation/callers move. | Keep platform/Main delivery separate; do not remove their implementation by deleting the shared host module wholesale. |
| Runtime deployment contracts, matching build numbers and qualification ladders (`agent-cli/src/host/platform_deploy.rs`, `installed_layout.rs`) | Do not port into development tooling. | Keep only contracts required by production platform delivery under that workflow's owner. |
| Legacy UI test input bridge (`apps/mister/ui_tests/driver.py`, `agent_input.py`) | Replace needed journeys with ordinary Slint events/assertions through 2.0. | The new Settings journey uses the existing application action queue. Do not copy the driver, input-correlation machinery or entire scenario catalogue. Physical controller health is a different claim. |
| Profile/route cross-product (`apps/mister/ui_tests/tests/test_profile_matrix.py`) | Drop from everyday development. | CI may retain narrowly justified hardware/layout coverage; no automatic local matrix. |
| Particle and scene experiments (`agent-cli/src/host/live_particles.rs`, `startup_particles.rs`, `agent-cli/src/commands/device.rs`) | Default to deletion. | Rescue an experiment into Mini only if it answers a current question; no command-by-command port. |
| Dedicated frame-profile runners/reports (`scripts/bench/reports/`, `agent-cli/src/host/performance_attribution.rs`) | Drop duplicate orchestration. | Keep a particular offline analysis only if someone uses it; pytest results and optional profiles are the normal evidence. |
| Workflow database commands (`agent-cli/src/cli.rs`, `DbCommand`) | Do not port. | Development results remain files. Retain only independently needed release/report consumers until their owner is established. |
| Desktop dashboard, SD browsing, framebuffer and telemetry (`apps/desktop/src/agent_client.rs`) | Active legacy consumer: retain temporarily, outside this milestone. | It imports `crates/agent-protocol`; decide which desktop features are wanted before replacing its client. The 2.0 viewer already covers development observation; do not recreate the entire dashboard in it. |
| Legacy agent ARM artifact (`scripts/magik_ci/build.py`, `.github/workflows/rust-arm.yml`) | Retain temporarily, then delete with the last installed consumer. | CI still builds/uploads the old binary. Removing runtime users alone does not remove this dependency. |
| Distribution artifact processing (`scripts/magik_ci/distribution.py`) | Keep outside 2.0. | It recognizes agent artifacts; production packaging needs an explicit consumer decision, not reuse of development deployment logic. |
| Legacy installation/startup (`agent-cli/src/host/agent_client.rs`, `/etc/init.d/S00magik-agent`) | Explicit later uninstall. | This milestone stops the process once, without editing boot configuration. An ordinary reboot may restart it. |
| FPGA signoff, CRT/latch qualification (`agent-cli/src/fpga.rs`, `host/crt_qualification.rs`, `host/latch_v5_qualification.rs`) | Keep hardware/release work outside 2.0; drop obsolete experiment-specific gates. | Never make these prerequisites for normal application deployment. |
| Catalog/media publication (`agent-cli/src/cli.rs`, `GameDatabaseCommand`, `host/media.rs`) | Keep data publication separate. | The runtime catalog itself is application functionality, not obsolete orchestration. No publication pipeline migration in this milestone. |
| Main/FPGA, RGB565 presentation and live application data | Preserve. | These are not legacy tooling merely because they predate 2.0. |

## What may be deleted next

Start with unused particle/scene commands, duplicate benchmark entrypoints and
obsolete local matrices, checking their direct callers before deletion. Do not
archive their source in another directory; Git preserves it. Move no new feature
into 2.0 as part of such a deletion unless a current consumer needs it.

The whole old agent cannot yet be uninstalled without resolving desktop and
production/CI consumers. Successful app development with that process stopped
establishes independence for the new development path only.

## Evidence caveat

Earlier `legacy_agent_running=false` results used an incorrect full-name match
against Linux's 15-byte `comm` field. They cannot establish absence. Milestone 3
corrects that check, verifies the actual executable before its explicit one-shot
stop, and records fresh before/after evidence. Neither normal delivery nor tests
automatically stop the old agent.
