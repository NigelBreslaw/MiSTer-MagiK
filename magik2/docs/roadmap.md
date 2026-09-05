# Document 1 — MiSTer MagiK Tooling 2.0 Roadmap

## Purpose and boundaries

Build a separately owned replacement for the development tooling: deployment, device control, live observation, testing, and profiling.

Keep the real MagiK application and installed Main/FPGA platform. Replace the old host/device tooling and retire obsolete experiments. The first version operates only on a tiny Slint experiment.

2.0 lives in an isolated `magik2/` project in this repository. It has a Python host, a small Rust native device agent, and its own dependencies, tests, and documentation.

### Operating principles

- **Development comes first.** Dirty worktrees and experimental branches are normal inputs.
- **Native transport is the normal path.** Preserve fast binary transfer and live streaming. SSH is only for automatic bootstrap or repair.
- **Check capabilities, not matching versions.** Leave a suitable installed agent alone. Automatically install missing support and continue.
- **Deploy only what was requested.** No implicit platform, database, or unrelated dependency updates.
- **Use one scenario system.** Python tests express behavior, collect benchmark measurements, and optionally enable profiling.
- **Keep recovery modest.** Use complete uploads, checksums, atomic file replacement, process ownership, and useful errors. Do not build snapshot orchestration or automatic reboot recovery.
- **Maintain tooling deliberately.** Core changes require dedicated tooling PRs. Application experiments consume its interfaces.

## Milestones

| Milestone | Deliverable | Completion condition |
|---|---|---|
| **1. Prove the small development loop** | Tiny Slint app; native deployment; live metrics/frames; shared tests and benchmarks; CPU profiles | Meets Document 2’s definition of done without invoking the old agent CLI |
| **2. Support real application experiments** | A small project descriptor and application adapter allow selected MagiK branches to use 2.0 | Real app changes deploy independently of unrelated checkout changes and agent build numbers |
| **3. Replace everyday 1.0 usage** | Real MagiK deployment, a small useful scenario suite, logs, profiles, and the new viewer | Normal development uses 2.0; retained workflows have named owners |
| **4. Retire specialized legacy workflows** | Classify remaining tools as replace, retain elsewhere, or delete | Every remaining 1.0 command has an explicit disposition; no automatic feature-parity requirement |
| **5. Switch consumers and documentation** | Update application integrations, CI, entrypoints, and repository instructions | No active consumer requires the old CLI or device agent |
| **6. Delete 1.0 tooling** | Remove obsolete code, dependencies, tests, commands, and documentation | Repository and device acceptance succeed with the old tooling absent |

Each milestone should be several reviewable PRs. The first is specified below; later milestones require their own bounded implementation plans.

### What to keep, replace, and discard

| Treatment | Features |
|---|---|
| **Keep the capability** | Fast native upload, streaming metrics/logs/frames, application lifecycle, profiling, meaningful hardware presentation evidence |
| **Reuse narrowly** | Existing hardware presentation/contracts, stable framebuffer codec, Slint Python client, proven ARM profiling workaround |
| **Replace** | Host orchestration, agent lifecycle/control, compatibility policy, deployment bookkeeping, test orchestration, benchmark reporting |
| **Default to deletion unless actively needed** | Historical catalog prototypes, fixed experiment matrices, duplicated profiler-specific runners, redundant qualification ladders, scenario-specific command families |
| **Keep outside the new development tool** | FPGA synthesis, platform release production, downloader publication, large data-release pipelines |
| **Preserve** | Real MagiK application, Main/FPGA platform, and shared hardware contracts still required by them |

A removed experiment remains recoverable from Git history. Do not copy obsolete implementations into an archive directory.

Hardware ABI identifiers remain hardware details. Their existence must not force matching versions between the host tool, device agent, and application.

## Ownership and eventual deletion

Separate the **tool core** from its **consumer experiment and scenarios**. Ordinary experiment PRs may change the latter without changing the agent or host implementation.

Add scoped instructions, code ownership, a tooling PR template, and a CI scope check. Core changes belong in a dedicated tooling PR; accompanying protocol tests, documentation, and probe changes are allowed. These review rules never run as deployment gates.

Before deleting 1.0:

1. Inventory references from applications, desktop Analytics, CI, scripts, manifests, and repository instructions.
2. Move only currently needed behavior to its appropriate owner.
3. Update real-app installation metadata coherently when its deployment path changes.
4. Remove legacy device-agent startup references and obsolete installed files through an explicit migration.
5. Delete the old host CLI, device-agent implementation, obsolete Python bridge/runners, experiment code, and unused dependencies.
6. Verify from a checkout without those sources, with the legacy device agent stopped or absent.

Deletion is complete when 2.0 can bootstrap, deploy, observe, test, and profile the supported application without invoking or depending on 1.0 tooling. Shared hardware libraries are not classified as obsolete merely because they predate 2.0.

---
