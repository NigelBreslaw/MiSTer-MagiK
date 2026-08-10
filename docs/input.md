# Unified input

Application navigation has one production contract: Main's input proxy protocol
v2. There is no protocol-v1 compatibility mode and no raw joystick or keyboard
fallback. If Main does not advertise `MISTER_MAGIK_INPUT_PROXY_PROTOCOL=2`, the
proxy is missing, or capture reports a fault, navigation is inhibited and the
launcher displays a recovery notice.

## Ownership

Main resolves its controller database and keyboard mappings, aggregates every
contributor to each logical action, and writes only aggregate `0 -> 1` and
`1 -> 0` transitions to its evdev proxy. Main does not generate launcher repeat.

The launcher has one blocking capture thread. It exclusively owns the proxy
file descriptor, drains ready events before discovery work, and publishes an
atomic `InputBatch` containing a contiguous event range, held state, topology,
and health at one sequence watermark. Its bounded critical journal has 1,024
records. Raw controller, keyboard, mouse, and analog observations are separate
setup, activity, or diagnostic data; they cannot navigate the application.

The UI loop drains the hub before catalog, media, Slint, or rendering work. The
drain returns an opaque mailbox observation that the idle wait must present
again. The wait sleeps only if no input, topology, or fault change occurred
since that exact drain, so input arriving during frame work cannot become a
lost wakeup. One
`InputRouter` owns focus priority, press-to-release capture, context generations,
source epochs, transition consumption, opposing-direction neutral locks, and menu
repeat. Menu repeat is immediate, then 300 ms, then every 80 ms. Home and Arcade
retain their continuous motion policies. Integrity faults clear router state and
require a neutral batch before recovery.

Authoritative selection changes immediately. A small Rust-owned feedback state
machine separately acknowledges eligible discrete focus destinations. It keys
entries by stable surface and item identities, permits overlapping pulses, and
never queues selection behind feedback. An acknowledgement's 80 ms clock starts
only when the exact submitted frame is confirmed as the active protocol-v5
latch sequence; removal is likewise complete only when its later frame is
physically confirmed. Re-entering a destination rearms it, while releases,
boundaries, swallowed input, asynchronous state changes, and Arcade's
fixed-selector velocity list do not create acknowledgements.

Launcher navigation transitions are 300 ms. Back and Home reverse an active
transition immediately. Every other press received while a transition owns
focus is consumed; it is never cached or replayed on the destination screen.

Events are routed and applied one at a time. Focus is recomputed after each
event, so a modal opened by one event can receive the next event from the same
captured batch. Reducers receive ordered actions and held ticks; production
navigation does not reconstruct edges from per-frame snapshots.

Focus priority is: disabled input, screensaver, lifecycle dialog, controller
setup, launcher modal, transition, diagnostic view, then the active screen.
Every accepted press remains captured by the context that received it until its
matching release, even if focus changes in between.

Automation and macOS preview input use the same logical event, phase, press ID,
source epoch, and router contract. The external automation request and response
schemas and its presented-frame acknowledgement remain unchanged.

Controller setup continues to use raw diagnostic input, but all setup reads and
writes target a stable physical-plug ID plus a connection generation. A
disconnect invalidates that exact target and cancels setup; reconnecting requires
a fresh press and cannot apply a stale write to a reordered `jsN` node.

## Qualification

Run the typed installed-runtime gate only against a clean, coherently delivered
Dev commit:

```bash
scripts/agent benchmark input-integrity
```

The gate keeps one bounded qualification-only uinput device alive for each
scenario. Linux delivers 100 taps cycling through 5, 10, 20, and 40 ms, a rapid
burst, and a 500 ms hold to Main. Main applies the real mapping and contributor
aggregation, and the resulting actions travel through proxy v2, kernel evdev,
`InputCapture`, and `InputRouter`. It runs once idle and once during a forced
catalog refresh, CPU contention, and a deliberate 500 ms UI stall.

The launcher records a bounded event trace. The gate requires the exact ordered
press/release pairs, matching press IDs, final neutral state, working menu
repeat, bounded queue use, healthy idle dispatch latency, and zero proxy write
failures, journal overflows, or sequence gaps. The intentional stall may delay
dispatch; it may not lose or duplicate input. Rendering cadence is reported but
is not an input-correctness gate—cadence qualification belongs to the rendering
benchmarks. The JSON report is stored under
`build/agent-benchmarks/input-integrity/`.

Run the visible-response gate after any launcher input, catalog, transition, or
presentation change:

```bash
scripts/agent benchmark launcher-response
```

It drives Computers from Acorn through Other at 100/50/57/64/71 ms schedules,
plus System Hub, Settings, and Arcade press-to-motion routes. Every eligible
discrete destination must have exact active-latch pulse-on and pulse-off
confirmations separated by at least 80 ms, while final selection remains
immediate and Arcade velocity motion remains pulse-free. Idle and forced-catalog
runs execute at 60 Hz and 50 Hz. Dispatch P95/maximum are gated at 3/5 ms;
input-to-visible median is gated at 12 ms and P95/maximum at the measured refresh
period plus 3/8 ms. The schema-v2 report separately states input-response,
pulse, integrity, and background-adoption results and requires zero logical,
mailbox, latch, or protocol-v5 physical faults. Evidence is stored under
`build/agent-benchmarks/launcher-response/`.

For causal diagnosis of intermittent single-press delay, run the dormant
on-device laboratory after delivering the exact clean commit:

```bash
scripts/agent benchmark input-latency-lab
```

This fixed diagnostic uses 1920×1200p60 and the production
uinput→Main proxy→InputHub→LauncherNav→Slint→RGB565→protocol-v5 path. It runs
the 64-press Acorn→Other→Acorn route independently under baseline, forced real
catalog work, monolithic 16/64 ms UI-thread work, and equivalent 64 ms work
split into 2/1 ms cooperative quanta. Every press has driver emission,
capture, drain, dispatch, projection, raster, post, and active-sequence
timestamps. Complete per-arm JSON is retained under
`build/agent-benchmarks/input-latency-lab/`.

The laboratory is armed only by a consumed `/tmp` session token and one-shot
launcher configuration. It restores the previous display mode and clears its
volatile state on success, failure, or timeout. It diagnoses scheduling and
presentation mechanisms; it does not replace `launcher-response` release
qualification or the attended human button-to-photon pass.

The automated gate does not replace the attended controller pass. Before
release, verify custom Main mappings, overlapping inputs from two controllers,
keyboard input, hotplug, setup, rapid taps, held scrolling, lifecycle dialogs,
and navigation transitions.
