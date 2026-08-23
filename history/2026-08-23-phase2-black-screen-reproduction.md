# Phase 2 physical black-screen reproduction

## Result

On 2026-08-23 the staged `launch-return-once` campaign reproduced the
showstopper with the Morph 4K powered and the fixed `USB Video` path proven
healthy. The failing cycle produced a 1920x1080 physical frame whose luma was
uniform video-level black:

```text
minimum=16 maximum=16 mean=16 visibility=black
```

At the same checkpoint, the authoritative internal RGB565 framebuffer showed
the complete settled Super Nintendo system view. Main, launcher, scanout-slot
latch, ownership, and vblank telemetry all remained coherent. A second bounded
visible-frame check timed out after three seconds, proving the physical black
was persistent rather than one transient capture frame.

The campaign stopped immediately. No further transition, launcher restart,
RBF reload, or recovery action was performed before the diagnostic snapshots.

## Capture-chain preflight

An earlier run was invalid because the Morph 4K had been powered off and its
USB output supplied fixed signal-loss bars. After it was powered on, the same
fixed camera produced a clean SNES MagiK image. The first ten valid physical
checkpoint images were byte-identical with SHA-256:

```text
4e08c6869c81fb9826d5c1b3135614b7865e08af8765b175d9d06b315f128071
```

This establishes a working capture path immediately before the genuine black
frame. The host classifier also rejects the Morph's unavailable-input bars as
`signal_lost`, so they cannot qualify as visible video.

## Campaign sequence

- Four valid transitions passed.
- A fifth harness attempt stopped before completing because the automation
  channel returned `EAGAIN` and then refused cleanup. It does not count as a
  transition result.
- Typed diagnosis required one rollback-capable delivery transaction. It
  reused the exact cached `platform-v0.29` bundle, performed one bounded reboot,
  and returned healthy.
- Ten consecutive post-recovery transitions passed with a five-second
  transaction cooldown.
- The next transition produced the persistent physical black frame and failed
  closed.

Therefore there were 14 valid passes in total, with the genuine failure on the
15th valid transition. Within the uninterrupted post-recovery boot epoch there
were ten passes followed by failure on the 11th transition.

## Exact identity

- application revision:
  `25f1fd8a2db21651557fdf64a9d011afbb6a9207`
- application build: `0.2.5187`, build 5187, clean ARM build
- Main PID/generation: `5868` / `375816`
- launcher PID: `5892`
- platform release: `platform-v0.29`
- platform bundle ID:
  `67c943bddf3325f82d6e6666f6046b16dab9d5a972295b0167054b181443170e`
- RBF SHA-256:
  `7484e004b3c6e089d9d377658633e435703bc1a224943b06215df9a9bccef4e7`
- platform manifest SHA-256:
  `87d0fd7c8314b5f5154d06122bd28a7ba9ca42fdd0aec3d3149490d61257f215`
- latch protocol/capabilities: `5` / `0x03ff`

The installed FPGA is the scaler-completion repair candidate. No new RBF was
built or installed during this campaign.

## Physical and internal evidence

The failed benchmark artifact directory is retained locally at:

```text
build/agent-benchmarks/launch-return-once/1787459360
```

| Artifact | SHA-256 | Result |
| --- | --- | --- |
| `summary.json` | `7ec5704b7fbf6ac301ab5a1900512fc7b9b1a2bc7ae1c24f1ff9fa4cd84d3da7` | failed closed |
| `returned-usb-video.jpg` | `dc4ee4f1eb9ede8c4b031b29fc8ba97d72068a296fb716cec2de087b6b25255e` | uniform physical black |
| `returned-framebuffer.png` | `288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250` | correct Arcade return |
| `snes-view-framebuffer.png` | `66314b11ea3affacc297982b9f0c94376f1ad22f7bf0e83f26351ee926cd4ede` | correct settled SNES view |

The images and full benchmark directory remain ignored local evidence and are
not committed. The exact read-only FPGA records are committed alongside this
note:

- `2026-08-23-phase2-black-screen-fpga-1.json`
- `2026-08-23-phase2-black-screen-fpga-2.json`

## Preserved runtime state

At the first snapshot:

- `launcher_state=LauncherActive`, `fpga_owner=magik`, owner epoch `1`;
- `present_backend=fpga-vblank-latch-hidden`, `present_status=ok`;
- 960x540 RGB565 route with 1,920-byte stride;
- latch sequence/flip count `301`, drop count `0`;
- zero crashes, restarts, invariants, blocked SPI writes, and blocked GPO
  writes;
- FPGA diagnostic classification `repair_transport_ready`, coherent state,
  and `sink_visibility=unobserved`.

## Diagnostic delta

The two snapshots were 31.19 seconds apart in device monotonic time.

| Field | First | Second | Delta |
| --- | ---: | ---: | ---: |
| owned vblank count | 3,176 | 5,047 | +1,871 |
| presented vblank count | 301 | 301 | 0 |
| repeated vblank count | 2,875 | 4,746 | +1,871 |
| active sequence | 301 | 301 | 0 |
| post count | 301 | 301 | 0 |
| flip count | 301 | 301 | 0 |
| drop count | 0 | 0 | 0 |
| reject count | 0 | 0 | 0 |
| ownership loss count | 0 | 0 | 0 |

Owned vblank continued at approximately 60 Hz. The static UI legitimately did
not publish another source frame, while the physical sink remained black.

## Source hashes

| Snapshot | File | SHA-256 |
| --- | --- | --- |
| first | `fpga-video-diagnostics.json` | `4ec6c18fd5f6efd74e1bc7d8dd572f499326ecac94924ce2b02e589df89617df` |
| second | `fpga-video-diagnostics.json` | `2ab79229ae7231957516578da9fff9b0b47df81e8437caf021d326e378cfc3a1` |
| first | `main-status.json` | `cc533e18007572064ee45754d4311f0faff53fed94cb5491bc3cefa8c9725798` |
| second | `main-status.json` | `dadaebd3383da9581ce7996eab9a6d5d27f9e66a2fbf44f662d6eca83239da7f` |
| first | `slint-status.json` | `a7d42ffd56357376d06ab1c814d42df8f5a4f1bc3f3ca3a4f8dd640699533f59` |
| second | `slint-status.json` | `06365d5e1782fe7176912175d16cea3254a9133482f004fb5d4703fecacbbf64` |

The committed FPGA JSON files differ from their source only by a terminating
newline.

## Conclusion and next diagnostic scope

`platform-v0.29` fails Phase 2 and cannot be commercially qualified. The
queued completion repair may have reduced the failure rate, but it has not
eliminated the physical black-screen defect. The current RBF cannot identify
the internal failing boundary because its observer fields are compatibility
stubs and `repair_transport_ready` observes only latch/ownership telemetry.

The next candidate should add one passive, read-only diagnostic block without
changing the latch protocol, framebuffer route, scaler scheduler, reset
behavior, or pixel output. The minimum incident evidence is:

- scaler fetch request, acceptance, return, and completion-credit progress;
- scaler HDMI-domain progress and output DE/HS/VS counters;
- final pre-TMDS RGB/DE/HS/VS fingerprints;
- HDMI clock, PLL-lock, output-enable, reset, and transmitter-state snapshots.

That RBF must pass the existing local simulation, CDC, timing, and Apple
container signoff gates before attended experimental installation. The next
transition campaign should not resume on `platform-v0.29`.
