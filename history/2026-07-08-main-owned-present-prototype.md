# 2026-07-08 Main-Owned Present Prototype

Prototype branch: `nigel/main-owned-present-prototype`.

## Goal

Learn whether a Main-owned hidden-buffer route changes the Arcade scroll frame
pacing failure shape. The prototype keeps Rust rendering/copying to `/dev/fb0`,
then sends `mister_magik_present_flip_v1 1` so Main copies the RGB565 frame into
hidden buffer 1 and routes scan-out there.

## Validation

- Main parser/unit smoke: `scripts/test-magik-state.sh` passed.
- Rust host check: `scripts/dev-rust check` passed.
- Profiler self-test: `scripts/profile-main-flip-present.sh MAINFLIP-SELFTEST --self-test` passed.
- ARM/device deploy: `scripts/deploy-main-mister-experiment.sh` passed.
- No persistent launcher/fault arming files remained after the run.

## Real-Hardware Benchmarks

Scenario: `turbo-hold`, 30s Arcade direct profile, bench-tools build.

| Label | Backend | Gate | p99 work | p99 wall | max wall | >16.667ms | Notes |
| --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| `PROTO-BASE-20260708B` | current cached present | fail | 5,171us | 16,985us | 17,993us | 496 | `vsync=1767`, no fallback/timeout/error |
| `PROTO-MAINFLIP-20260708` | `main-flip-v1` buffer 1 | fail | 5,386us | 16,985us | 17,136us | 444 | `vsync=1767`, no fallback/timeout/error |

Artifacts:

- `build/arcade-scroll-profiles/PROTO-BASE-20260708B-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PROTO-BASE-20260708B-arcade-scroll.log`
- `build/arcade-scroll-profiles/PROTO-MAINFLIP-20260708-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PROTO-MAINFLIP-20260708-arcade-scroll.log`

## Result

The prototype did not pass the strict frame pacing gate. It did reduce the
post-warm frames over 16.667ms from 496 to 444 and reduced max post-warm wall
time from 17,993us to 17,136us, but p99 wall time stayed at 16,985us. Work time
remained far below budget in both runs, so the failure shape still looks like
phase/wake timing rather than CPU saturation.

The prototype adds about 87us average command-write accounting and a second
full-frame byte count in the Rust trace. The Main-side copy itself is not
acknowledged back to Rust in this prototype, so a follow-up would need an ack or
shared timing stamp before treating Main copy cost as fully measured.
