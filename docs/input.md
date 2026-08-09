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
source epochs, transition queuing, opposing-direction neutral locks, and menu
repeat. Menu repeat is immediate, then 300 ms, then every 80 ms. Home and Arcade
retain their continuous motion policies. Integrity faults clear router state and
require a neutral batch before recovery.

Focus priority is: disabled input, screensaver, lifecycle dialog, controller
setup, launcher modal, transition, diagnostic view, then the active screen.
Every accepted press remains captured by the context that received it until its
matching release, even if focus changes in between.

Automation and macOS preview input use the same logical event, phase, press ID,
source epoch, and router contract. The external automation request and response
schemas and its presented-frame acknowledgement remain unchanged.

## Qualification

Run the typed installed-runtime gate only against a clean, coherently delivered
Dev commit:

```bash
scripts/agent benchmark input-integrity
```

The gate creates a bounded qualification-only uinput device. Linux delivers its
events to Main, Main applies the real mapping and contributor aggregation, and
the resulting actions travel through proxy v2, kernel evdev, `InputCapture`, and
`InputRouter`. The run exercises 5, 10, 20, and 40 ms pulses, a burst, and a
deliberate 500 ms UI stall. Evidence is rejected for any lost or duplicated
action, proxy write failure, journal overflow, sequence gap, dropped frame, or
latch drop. The JSON report is stored under
`build/agent-benchmarks/input-integrity/`.

The automated gate does not replace the attended controller pass. Before
release, verify custom Main mappings, overlapping inputs from two controllers,
keyboard input, hotplug, setup, rapid taps, held scrolling, lifecycle dialogs,
and navigation transitions.
