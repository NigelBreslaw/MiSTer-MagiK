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

Launcher navigation transitions are 300 ms. Every press received while a
transition owns focus is consumed, including Back and Home; it is never cached
or replayed on the destination screen. Matching releases remain captured by
the transition and cannot leak into the destination. Transition reversal is
reserved for internal rollback after preparation failure or cancellation.

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
