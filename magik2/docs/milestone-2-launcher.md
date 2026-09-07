# Mini launcher milestone 2: browsing delivery

## Scope and preserved baseline

This is still Mini, with fixture catalog data, not the real launcher integration.
Categories remain Arcade, SNK NeoGeo, Consoles, Handhelds and Computers, initially
Arcade. Library totals 6842 / 18 / 126 and clock 21:37 are fixed fixtures; only
Arcade's requested 1752 count has user-supplied provenance. Other counts remain
test fixtures. No controller button launches a game in this milestone.

The baseline cleanup was committed as `3e0834aad`: pure-black background, removed
Settings/tagline/sidebar slogans/offline copy, and short 32-pixel reflections with
approximately 22% initial brightness, quadratic fading and fixed ordered dither.
Do not restore the former four-band reflection or removed copy.

The source/destination fix remains intact: Main display state resolves the plan;
the existing HiddenLatchPresenter programs the FPGA destination. At the accepted
1080p mode the UI source is 960x540 and the destination is 1920x1080. Native PNGs
capture the source, not the HDMI output after scaling.

## Interaction contract

- Right selects the next category while cards move left; Left reverses that.
- An immediate tap starts a 180ms slide. A hold starts subsequent 150ms slides
  after 300ms from the genuine press. Release completes the current slide only.
- One latest discrete pending tap is retained; repeat events cannot build queues.
  Opposite input finishes the current slide then follows the latest intent.
  Both directions held prevent new movement. Large time gaps cannot skip through
  multiple unseen cards.
- Mode changes and input loss clear held/pending intent. The mapped reader takes
  a kernel held-key snapshot on open and waits for neutral after inherited holds.
- Every transition in this milestone is a slide, including fresh presses.
  Same-direction paired perspective flips and their preferred reverse mapping
  remain milestone 3; catalog/game launching remains milestone 4.

## Implementation and review

Luna at medium reasoning implemented the portable browser and renderer. The
coordinator returned issues found in timing, endpoints, clipping and caching, and
corrected the consumer/runtime input and final-presentation integration directly.
Independent integration tests compare all category endpoints against the accepted
static renderer, require an unchanged sidebar during movement, and count heap
allocations inside the prepared renderer.

Main's existing v2/v3 virtual EV_KEY stream is the only physical input authority.
There is no guessed raw-joystick fallback or Main change. The later approved
tooling correction reuses real MagiK's existing supervised launch path for Mini;
see `mini-main-supervision.md` for source provenance and evidence.
The runtime adapter drains bounded reusable batches, preserves partial input
records, ignores repeats/invalid values, and discards a batch on SYN_DROPPED or
transport loss. Mini reconnects without exiting or spinning a recovery loop.

Slint is only the retained test shell. Invisible down/up/reset actions feed the
same portable browser as physical input; source-specific held state is merged so
a test release cannot release a physical hold. Probe demo callbacks are mode
gated. Settled readiness and selection are published only after a successful
physical latch, with pending settlement retried before writing another slot.

## Focused validation

Commands (run from repository root):

```sh
scripts/cargo test --manifest-path magik2/probe/Cargo.toml --locked -p mister-magik-framebuffer-scenes
scripts/cargo test --manifest-path magik2/probe/Cargo.toml --locked -p mister-magik-mister-runtime --lib main_input::tests
scripts/cargo test --manifest-path magik2/probe/Cargo.toml --locked -p mini-magik --bin mini-magik
scripts/cargo clippy --manifest-path magik2/probe/Cargo.toml --locked -p mini-magik --bin mini-magik -- -D warnings
magik2/host/.venv/bin/python -m pytest magik2/scenarios/test_mini_display.py magik2/scenarios/test_mini_motion.py --magik2-app mini-magik -q
scripts/magik2 deploy --app mini-magik
scripts/magik2 check --app mini-magik
scripts/magik2 check motion --app mini-magik
```

The last command intentionally retains the two separately named probe samples
and adds two launcher samples, one per direction. Launcher measurements use a
2s warmup and 5s device-clock window. No accessibility polling or capture occurs
inside that window. Launcher events use `phase=launcher-motion` so the retained
probe summary cannot pool these workloads. Physical repeats are reported
separately from dropped/rejected posts, with FPGA ownership/invariant checks.

## Device acceptance record

Deployment and smoke passed: `20260907T205835Z-0226faca5b1f` and
`20260907T210355Z-f21739e0bc86`. Main is active and supervises Mini with the same
960x540 source / 1920x1080 destination. The user confirmed all physical joystick
movement works correctly on 2026-09-08.

Motion qualification remains open: `20260907T210844Z-d170188add21` fails the
retained probe workload's presentation accounting gate, before launcher samples.
This is not evidence that the launcher's animation has passed cadence validation.
Milestone 3 is explicitly approved as the next visual implementation, not a
retroactive performance qualification of milestone 2.

Screenshots, timing logs and runtime artifacts remain untracked. No push is part
of this delivery.
