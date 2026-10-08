# Plan: shrinking `run_launcher_loop`

Status: plan only. Nothing in this document has been implemented. Every number below was measured on
`main` at ae446e182 (`apps/mister/src/ui_runner/launcher_loop.rs`), by static analysis of the source plus one
throwaway compiler run (see "Method and limits"). The PR 29 entry in
[presentation-refactor-roadmap.md](presentation-refactor-roadmap.md) records why a loop shrink was
deferred. This plan is the answer to that deferral: what the function actually contains, what is
dead, what is repeated, and a staged order that never moves more than the evidence supports.

## 1. What the function is

`run_launcher_loop` spans lines 5005 to 13581: **8,577 lines**. It is a setup section of 1,208 lines
followed by one `'launcher: while` loop of 7,368 lines.

| Measure | Value |
|---|---|
| Locals declared in setup | 221 |
| Locals declared per frame (loop top level) | 288 |
| Statements in setup / in the loop body | 314 / 645 |
| `#[cfg(feature = "tooling")]` attributes inside it | 94 |
| Statements gated by that feature, and their size | 88, about 744 lines |
| Lines that mention bench, tooling, trace, profile, analytics, automation, fixtures or capture | 1,029 (12.0%) |
| Early exits (`continue 'launcher`) | 16, of which 12 are inside the input phase |
| Macros that capture locals implicitly | 4 (`request_launcher_redraw!` 66 uses, `record_launcher_frame_phase!` 22, `note_pre_input_boundary!` 12, `render_launcher_base!` 5) |
| `print_startup_event` / `ui_errln!`/`ui_println!` calls | 41 / 54 |
| Mutable `bool` flag locals | 41 |

### 1.1 Setup locals, by how many loop statements use them

| Referenced by | Locals | Meaning for extraction |
|---|---|---|
| 0 loop statements | 46 | Startup-only. Leave the function with the setup. |
| exactly 1 | 53 | Belongs to that block's owner. Move with it. 17 of them belong to the input phase alone (`input_router`, `ui_action_sequence`, `ui_test_fixture`, `settings_store`, ...). |
| 2 to 3 | 45 | Small shared groups. |
| 4 to 10 | 47 | Shared, mostly by one phase pair. |
| more than 10 | **30** | The real shared state. |

The 30 widely shared locals, with how many loop statements use each: `nav` 92, `director` 48, `tooling` 46,
`preview` 46, `launcher_response_trace` 43, `catalog` 43, `start` 40, `lifecycle` 40, `layout` 38,
`screensaver` 34, `frames` 33, `navigation` 32, `frame_accounting` 31, `scheduler` 30, `run_start` 20,
`startup_intro` 19, `launcher_presenter` 19, `gui_profiling` 19, `catalog_ready` 19, `launcher_card_home` 18,
`catalog_version` 15, `pacer` 13, `arcade_entry_latency` 13, `media_session` 12, `lifecycle_effects` 12,
`catalog_session` 12, `screensaver_pipeline` 11, `launch_return_session` 11, `latency_critical_input_pending`
11, `input_observation` 11.

The earlier worry was that extracting a block means moving "dozens of locals". That is true of the
**interface of a block** (the input phase touches 79 setup locals) but not of the **state**: 191 of the 221
setup locals are used by at most ten statements, and 99 of them by at most one.

### 1.2 The frame has seven phases, and the function already marks them

`record_launcher_frame_phase!(LauncherFramePhase::...)` divides each frame. Sizes and coupling of each:

| Phase | Lines | Per-frame locals it declares | Of those, used by a later phase |
|---|---|---|---|
| 0 begin, catalog events, launch recovery (6213) | 1,295 | 55 | 29 |
| 1 input (7508) | 1,379 | 2 | 2 |
| 2 background work, previews, prefetch (8887) | 899 | 82 | 42 |
| 3 idle wait and pacing (9786) | 104 | 0 | 0 |
| 4 render and composition (9890) | 2,134 | 125 | 51 |
| 5 plan, submit, present (12024) | 468 | 20 | 14 |
| 6 post-submit, confirmation, accounting (12492) | 1,061 | 4 | **0** |

**138 of the 288 per-frame locals cross a phase boundary**, so a per-frame context is unavoidable, but it
is about 138 values, not 288, and they are concentrated in phases 0, 2, 4 and 5. Phase 6 is a **leaf**: it
declares four locals, none used afterwards, so extracting it creates no outgoing coupling. The input
phase also declares almost nothing that outlives it, but 12 of the 16 early exits are inside it, so it
needs an explicit outcome type.

The widest-crossing per-frame locals are `animation_now` (7 phases), `tooling_frame_evidence` (6),
`scheduler_phase` (6), `loop_start` (6), `launching` (6), then the catalog-scan and confirm-dialog flags
(4 to 5 each). The scan and dialog flags already travel together as 20-odd positional arguments to
`finish_frame_before_trace`.

### 1.3 Existing call sites already show the seams

Two functions take 22 and 20 parameters (`process_catalog_worker_message`,
`apply_catalog_session_effects`). Each of the **six call sites** inside the loop (two for the first, four
for the second: lines 6936, 6993, 7309, 8347, 8463, 8497) passes the same run of 14 shared `&mut`
references, `nav, catalog, catalog_ready, catalog_version, return_capsule_active, catalog_generation,
launch_return_session, preview, scheduler, lifecycle, lifecycle_effects, full_bridge_dirty,
startup_intro_catalog_ui_replay, startup_intro_catalog_shells_pending` (the worker-message calls add
`catalog_session`). That run is a struct the code has already named by repetition.
`finish_frame_before_trace` takes **29** positional arguments in a single call at 12,440, seven of
them `status_text.as_ref().map(|text| text.X.as_str()).unwrap_or("")` (21 in the function).

## 2. Deletion audit

Method: lift every `#[allow(dead_code)]` on `ui_runner` modules (19 suppressions in `ui_runner.rs`,
`launcher_present/mod.rs` and `launcher_screensaver.rs`, **including
`launcher_loop`, `launcher_frame_accounting`, `launcher_compositor`, `launcher_scheduler` and
`launcher_screensaver`**), plus the `cfg_attr` in `lib.rs`, compile with the `ui` and `tooling` features, then grep every flagged symbol
across the repository. The edit was reverted; the tree is clean. Result: **33 findings on the `ui`
build and 32 on `tooling`** (the one that differs is the latch `copy`/`copy_us` field pair, which tooling reads). The blanket suppressions are themselves the first finding: these modules
cannot currently report dead code on the host, and `launcher_loop` and friends are exempt on every target.

| Class | Items | Disposition |
|---|---|---|
| Never referenced anywhere | `restore_cached` (compositor), `is_loading_archive` (screensaver) | Delete. |
| Referenced only by tests (production API with test-only callers) | `pad_state_with`, `is_preview`, `wait_for_latch_completion_with`, `presenter_state_uses_latch`, `direct_hidden_framebuffer_geometry_available`, `failure_transitions` accessor, `media_worker_unavailable`, `current_frame_budget_status`, `catalog_persistence_failed_intent`, `media_progress_model` (its only caller is a `#[cfg(test)]` method), `try_issue_hidden_slot_render_grant` (the latch version, and the orchestrator wrapper that only forwards to it), `render_black`, `activate_fb0_route_with_hardware`, the scheduler's `new` (called at about 24 sites, all in tests) and `with_catalog_config` (called only by `new`) | Move under `#[cfg(test)]` with the tests that use them, or delete both. |
| Fields set but never read | `launch_ref`, `load_us`, `preview_blit_us`, `cards_adopted`/`cards_drawn`/`cards_culled`, `frame` (screensaver pipeline), and in one timing struct at `launcher_frame_accounting.rs:118`: `search_index_state`, `main_present_hidden_copied_bytes`, `home_pan_present_active`, `home_horizontal_input_held`, `arcade_update_label`, `status_string_copy_us`; plus `runtime_status_write_deferred` (field at 325) | Delete the field and its assignment. |
| Preview-only code compiled into the device library | `LauncherScreensaver` and its methods (`render_at`, `has_pending_card_work`, `active_card_count`, `from_archive_path`, `log_shared_parade_stats`) in `launcher_screensaver.rs` (524 lines). `git grep` finds the type constructed only in `ui_preview.rs` and its own tests; the loop and the screensaver pipeline never name it. | Confirm no device path reaches it, then move it behind `ui-preview`. |
| Enum variants never constructed in production | `LauncherBenchScenario::{Idle, PreviewIdle, HomeNav, HeldScroll, TurboHold, ScreensaverShow}` and `LauncherWorkerUiIntent::HideCatalogBackgroundScan` (constructed only by a loop test) | Investigate the scenario parser first (it is the only constructor); delete the intent and its two match arms if nothing emits it. |

**This audit does not shrink the loop much, and it would be wrong to claim otherwise.** I checked the
tempting case: `home_pan_present_active` and `home_horizontal_input_held` look dead because one
struct field is never read, but the loop locals of those names are live decisions (lines 9640, 10752 and
12996). Only the write-only telemetry copy goes. The audit's value is hygiene and honesty about what
the code does: about 40 items, most of them small, plus 524 lines of preview-only code that does not
belong in the device build, plus removing the blanket suppressions so dead code is reported again.

Caveats that apply to every row:

- The compile ran on the **host target**. A symbol flagged here can be used by `cfg(target_arch =
  "arm")` code. Stage D therefore verifies each deletion with the ARM build before it is removed.
- "Referenced only by tests" was decided by `git grep -w`, which also matches comments and
  common names (`new`, `frame`, `copy`, `empty`). Those names were reviewed by hand and the generic
  ones (`new` on the compositor, `frame`, `copy`) are not claimed here without a per-site check.
- `frame_rect` was flagged but is used by `tear_pattern_loop.rs` and `video_loop.rs`; it is not dead.

## 3. Simplification audit

| Finding | Evidence | Simpler form |
|---|---|---|
| The catalog argument run is repeated by hand | six call sites, a run of 14 shared references; two functions with 22 and 20 parameters; 14 `too_many_arguments` allowances in the file | One `CatalogDomain` borrowed struct. |
| One call with 29 positional arguments | `finish_frame_before_trace`, 12,440 | A `FrameFacts` struct built once from values the frame already holds. |
| The same string accessor written 21 times | `status_text.as_ref().map(\|text\| text.X.as_str()).unwrap_or("")` | A `StatusText::or_empty()` view with named getters. |
| Benchmark, tooling and trace policy interleaved with production decisions | 1,029 lines (12.0%), 88 tooling-gated statements, 94 cfg attributes | A `Tooling` hook object with no-op production methods; the loop calls hooks, never branches on policy. This is the rule `ui_runner/AGENTS.md` already states: "Isolate benchmark policy from production defaults." |
| Implicit capture through macros | `request_launcher_redraw!` 66 uses, `record_launcher_frame_phase!` 22 | Methods on the context that own the flag they set. |
| Locals that exist only to feed one block | 53 setup locals with exactly one user | Fields on that block's owner. |
| Startup-only locals living in the loop function | 46 setup locals never used by the loop | Locals of a startup function. |
| 41 mutable `bool` flags and 126 timestamp-style names | flag soup in a 7,000-line scope | Per-owner state structs (e.g. `StartupGate`, `ScanVisibility`); do this last, it needs domain reading. |

Not simplifications, deliberately left alone: the four macros' *behaviour* (only where they capture),
and the phase order, which is correctness-critical and device-verified.

## 4. The plan

Principle: **every stage moves code without changing behaviour, is sized by a measured line count, and
leaves the tree shippable.** A stage that cannot show its measured effect does not ship. No stage is
combined with a behaviour change.

### Stage D: deletions (independent, do first)
- Remove blanket `#[allow(dead_code)]` module by module as each is cleaned; end state: none in `ui_runner.rs`.
- Delete the "never referenced" items; move test-only items under `#[cfg(test)]`; delete write-only
  fields and their assignments; move `LauncherScreensaver` behind `ui-preview`.
- Resolve the bench scenario parser question and `HideCatalogBackgroundScan`.
- Gate: ARM build (CI) shows no new warning; host and tooling builds pass; `check journeys`.
- Measured effect to record: items deleted, suppressions removed, device-library lines removed.

### Stage 1: lift startup out of the function
Move the setup statements before line 6098 (about 1,090 lines) into `launcher_startup.rs` as
`build_loop_state(...) -> LoopState`. The loop function begins with
`let LoopState { nav, director, ... } = state;`, so **the frame loop body does not change at all** and no
call site changes. The 46 startup-only locals stay inside the new function and drop out of the struct
(about 175 fields remain). Two things stay in `run_launcher_loop`: the macros defined at lines 6098 and
6156, which capture locals by name and so must be defined after the destructuring, and the roughly 115
lines of setup that follow them. Setup has no function-level early return: the two `?` at lines 5548
and 5848 sit inside closures.
- Effect to record: `run_launcher_loop` shrinks by about 1,090 lines; the loop body diff is zero lines.
- Gate: builds, test suites, `check journeys`, and a cold-start comparison. Statement order is kept as
  it is, so startup event order is preserved by construction; I did not find a test that asserts it.

### Stage 2: the catalog domain
Introduce `CatalogDomain<'a>` holding the 14 shared references (15 with `catalog_session`) and change `process_catalog_worker_message`
and `apply_catalog_session_effects` to take it (22 and 20 parameters become about 5). Replace the six
hand-written argument lists. Move the 53 single-user locals into the owners they belong to as each
phase is extracted.
- Effect: two signatures and six call sites shrink; `too_many_arguments` allowances drop by at least 2.

### Stage 3: phase 6, the leaf
Extract the post-submit, confirmation and accounting phase (1,061 lines) as a method taking the loop
state and a read-only `FrameFacts` snapshot. It has no outgoing locals. The 29-argument call and its seven
`status_text` accessors disappear inside this stage (the other 14 accessors are elsewhere in the function).
- Effect: about 1,061 lines leave the function; one struct built once per frame.
- This stage is the proof of the approach. **If it needs more than about 25 fields in `FrameFacts`, or
  changes any frame-timing number beyond run-to-run spread, stop and reassess the whole plan.**

### Stage 4: isolate benchmark and tooling policy
Replace tooling-gated statements and bench branches with calls on a `Tooling` hook object whose
production methods are empty. Do this per phase as that phase is extracted, not across the function.
- Effect: the 88 gated statements (about 744 lines) and 94 cfg attributes leave the loop.

### Stage 5: the input phase
Extract phase 1 (1,379 lines) behind an `InputPhase` struct that owns its 17 single-user locals, returning
an explicit outcome (continue the frame / restart the frame) in place of its 12 `continue 'launcher`
exits. It declares only two per-frame locals, so outgoing coupling is small; the cost is entirely the
79-local interface, which Stages 1 and 2 will already have collapsed.

### Stage 6: phases 0, 2 and 5
Pre-input catalog and launch recovery (1,295 lines), background work and previews (899), and plan,
submit and present (468). These carry the 29, 42 and 14 cross-phase locals, so each is a method over the
loop state plus a growing `FrameState`. Extract in that order of decreasing independence.

### Stage 7: render and composition (last, and conditional)
Phase 4 (2,134 lines) declares 125 per-frame locals, 51 of them used later. It contains the 387-line
navigation-composition block, which already talks to the `PresentationDirector`. Only attempt it after
Stages 1 to 6, when the function is small enough to see what is left. It may stay a single method.

### Expected end state
Each phase a method, `run_launcher_loop` a frame skeleton plus the macros' replacements. I am not
promising a line count for the whole; each stage records its own measured result and the next stage is
sized from it.

## 5. Gates for every stage

1. `scripts/cargo test --features ui --tests` for `apps/mister`, `scripts/cargo test` for touched crates,
   clippy clean for `ui` and `tooling`, and the Python suites, all run in full.
2. `scripts/magik check journeys` on the deployed build.
3. For stages that touch the frame path (3 and later): `animation-roundtrip` on both orientations,
   comparing producer time and drop counts to a same-session baseline. The earlier A/B showed that
   three repetitions per cell cannot detect a few percent, and that session-to-session variation can
   exceed the effect, so a stage is judged on *no regression beyond the baseline's own spread*, and
   baselines are taken in the same session (interleaved).
4. Deletion/simplification audit of the stage's own diff, and a read of the full diff.
5. The function's line count and local counts recorded in this document's progress log.

## 6. Method and limits

- Local counts and usage come from identifier matching on the source, not from the compiler. A name that
  is shadowed or used only inside a macro body can be miscounted; the 46 "startup-only" locals in
  particular must be confirmed by deleting each from the loop's scope at compile time during Stage 1.
- The phase table splits the function at the `record_launcher_frame_phase!` markers (lines 6213,
  7508, 8887, 12024, 12492) and, for the idle-wait and render boundaries (9786, 9890), at statement
  starts I chose from reading the code, because no marker sits exactly there. Locals are attributed by
  line range, so a local first used in the next phase is counted as crossing.
- The dead-code run used the host target. See the caveats in section 2.
- This analysis did not run the device and makes no performance claim.

## 7. Progress log

| Stage | Status | `run_launcher_loop` lines | Notes |
|---|---|---|---|
| baseline (ae446e182) | measured | 8,577 | setup 1,208; loop 7,368 |
