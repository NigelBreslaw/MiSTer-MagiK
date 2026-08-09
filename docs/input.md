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

The UI loop drains the hub before catalog, media, Slint, or rendering work. One
`InputRouter` owns focus priority, press-to-release capture, context generations,
source epochs, transition consumption, opposing-direction neutral locks, and menu
repeat. Menu repeat is immediate, then 300 ms, then every 80 ms. Home and Arcade
retain their continuous motion policies. Integrity faults clear router state and
require a neutral batch before recovery.

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

It verifies launcher transition input, then sends 5/10/20/40 ms gamepad pulses
at 50 ms start-to-start spacing plus a repeat hold on the SNES system hub. The
input travels through Main's proxy-v2 and kernel path. Each accepted focus move
must reach a distinct physically confirmed frame in order. Every confirmed
response must be under 50 ms. The same run repeats
during catalog work, requires prepared catalog adoption below 8 ms, verifies
Back reversal and transition-time input consumption, and requires zero input or
latch faults. Physical cadence is qualified separately by
`navigation-transitions`. Evidence is stored under
`build/agent-benchmarks/launcher-response/`.

The automated gate does not replace the attended controller pass. Before
release, verify custom Main mappings, overlapping inputs from two controllers,
keyboard input, hotplug, setup, rapid taps, held scrolling, lifecycle dialogs,
and navigation transitions.
