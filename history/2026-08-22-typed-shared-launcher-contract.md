# Typed shared launcher contract closure — 2026-08-22

## Outcome

Plan 03 Alternative replaced the monolithic launcher bridge with typed Slint
domain globals shared by CRT and HDMI. The two launcher shells, component
trees, layout expression, typography, and visual behavior remain independent.
They now consume the same semantic data and named action contract.

The implementation sequence begins at `1a4cd19df` and the closing assurance
commit is `831497b6f`. The reviewed baseline commit is `be61719c8`; its manifest
pins source commit `9f030ee3ad0b8b649892d6367f341c9be4ce8dcb` and scene-manifest SHA-256
`d53a60a81a87d0bbe7d895028837dc379075d878e98b10b9bb188e4ebe86e896`.

## Semantic and retained-model evidence

- `NavigationView`, `SettingsView`, `InformationView`, `InputView`, `SetupView`,
  `ArcadeView`, `CatalogView`, `MediaView`, `OverlayView`, `FeedbackView`,
  `LauncherLayout`, and `LauncherActions` are exported from the shared API.
- Exhaustive Rust conversion tests cover existing domain variants. A final
  ratchet rejects the retired monolithic bridge symbols, legacy discriminant
  fields, integer finite state, and numeric comparisons against typed state.
- Production and preview exercise the same presenters and semantic fixtures.
  Retained menu, Arcade, media, settings, and license models preserve their
  minimal-delta update paths and churn accounting.
- Menu hierarchy and input-fault visibility use typed states. Breadcrumbs,
  fault text, stable IDs, and labels remain presentation data rather than state
  discriminants.

## Interaction evidence

- Named Slint callbacks decode into a bounded typed Rust action queue.
- Unknown or stale stable IDs are rejected without projected-state mutation.
- Queued UI actions enter the existing launcher input-routing phase and retain
  modal, setup, transition, screensaver, feedback, navigation, consumption, and
  recovery precedence with controller input.
- Fixture coverage includes simultaneous UI/controller actions, queue bounds,
  transition denial, setup/release association, overlays, and preview parity.

## Visual evidence

The accepted baseline contains 18 deterministic RGB565/PNG scenes: Home,
Arcade, Settings, controller setup, catalog scan, and navigation-transition
midpoint for HDMI, CRT 240p, and CRT 480p. The read-only comparison passed after
the final typed projection changes. Canonical host assurance selected 29 checks
and passed on 2026-08-22; an explicit baseline-scoped selection also passed.
CI retains expected, actual, and diff PNGs when comparison fails, while baseline
generation remains a separate explicit workflow and baseline-only commit.

This is host-rendered semantic, projection, composition, and RGB565 evidence.
It is not evidence that a physical HDMI or CRT sink displayed the same pixels;
device equivalence can be revisited as a separate attended task.

## Defects found and corrected

- CRT runtime modes excluded from the settings selector could be interpreted as
  filtered settings indices, producing the wrong active-resolution label.
- Preview did not project CRT `LauncherLayout` geometry after layout cutover.
- Shared orientation still used `0/1/2`; an unused System Hub icon selector used
  `0–3`; menu hierarchy and input-fault visibility still depended on strings.

All were corrected before the assurance ratchet was enabled.

## Deferred work

Runtime-environment closure (the former C20/Type-A lane) remains separate debt.
Launcher-loop observation/reduction/effect/composition/presentation
decomposition also remains separate debt. Neither was completed or partially
claimed by this migration. The two incomplete benchmark routes recorded in
`history/2026-08-15-p1-enforce-acceptance.md` remain visible pre-existing debt.
