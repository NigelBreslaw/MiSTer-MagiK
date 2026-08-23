# Operator-confirmed moving-band corruption after reboot

## Preservation status

On 2026-08-23 an operator-confirmed physical corruption occurred immediately
after a requested attended reboot. The host-side reboot command was interrupted
after 28.1 seconds, but fresh device identity proved that the reboot had
completed: boot ID `542`, Main PID/generation `579` / `7693`, and launcher PID
`614`. MiSTer MagiK was visibly running on Home with `LauncherActive` ownership.

After the operator identified the corruption, no return, restart, RBF reload,
additional reboot, or recovery operation was issued. The device remained in
the failing state while physical video, internal framebuffer, and two
independent read-only diagnostic bundles were captured. It was still
unrecovered when this record was committed.

The incident followed 55 uninterrupted, valid, post-return MagiK passes in the
preceding boot epoch. Those passes used the corrected attended harness and are
not failures. This occurrence is a post-reboot MagiK initialization failure,
not a core-active or core-loading image.

## Physical evidence

The fixed macOS-native `USB Video` input recorded the corruption at
1920x1080. The UI remained recognizable, but horizontal discontinuities and
displaced regions moved continuously from the top toward the bottom, wrapped,
and repeated. This is the rare moving-band corruption reported by the physical
operator; it is not a black screen or signal-loss pattern.

The preserved 30-second movie was decoded sequentially with AVAssetReader.
Every encoded frame was retained in presentation order: 732 frames spanning
`30.059833333` seconds, averaging `24.318` captured frames per second with a
median 40 ms presentation interval. No missing 60 Hz frames were synthesized;
the sequence is exactly the frame cadence delivered by the USB capture device.

Row-by-row change across all consecutive frames produced a 732x1080
time-versus-height heatmap. Its repeated diagonal trajectory independently
confirms that the corruption band moves down the complete physical image rather
than remaining at a fixed framebuffer address.

| Artifact | SHA-256 | Result |
| --- | --- | --- |
| 30-second native movie | `9384842b5620daf1e61a4ff8906821127ef949d12d2b95822bebd33df110663c` | persistent moving corruption |
| Initial 20-second native movie | `c992c1dbdf477fdca1e7b907c3cab45a34a48a9fca7a7060f9a94f78a0735cd5` | persistent moving corruption |
| Physical still | `c5712a30c6c2ee6742ff7ab6c8225dec60218f896d4488bb23f07d435e971e2a` | spatial corruption visible |
| Exact frame index | `1e54a3fd0b7ea42f55b4f34a740a42642464373f8d3304f8f9f2a10003be4808` | 732 ordered timestamps |
| Row-change heatmap | `f0bc4799b17dccedf7d74090ce6cf65917ec04e5d847785a3d20aefe6edc265e` | repeated top-to-bottom trajectory |
| Band peak table | `b2fb1b0d283554f6c51797a664dea51e3309010be91e803fdc80f43a5e015520` | per-frame row-change maxima |

The ignored local evidence remains under the common prefix:

```text
build/raw-scaler-phase2-screen-corruption-epoch2-
```

## Internal evidence

The authoritative RGB565 capture from the FPGA latched scanout slots was
complete and spatially correct while the physical output remained corrupt:

```text
009b2ad19587a5842c91aded2f1a0986c33a00e312371977e7cf151ac27bcde2
```

Both diagnostic bundles were captured without recovering the device. Their
raw-scaler records were coherent `raw_scaler_active`, with stable MagiK
ownership and advancing raw frame publication. The observer reports
`sink_visibility: "unobserved"`; the native USB recording is the physical
authority.

| Snapshot | Device monotonic interval | FPGA diagnostic SHA-256 | Bundle SHA-256 |
| --- | --- | --- | --- |
| A | `149.310–149.360 s` | `ccfb1cbe739b893d3a9da02630553b24d2c06f7142473c8b46775955dd410780` | `8b4720e5e17d944820c2be4d8e8e51380049060bf247eaf1fa529c3e202d8860` |
| B | `157.940–157.990 s` | `f99e30fcc90e034017dca1eebeaa57328296be71f3dc0063f4249d5ca8329f74` | `a146b810d555fe36c43a4e5b79a892da3ae368a12fd5222e8940c8231abd09da` |

At both snapshots:

- Main state was `LauncherActive`, FPGA owner `magik`, owner epoch `1`;
- the launcher used `fpga-vblank-latch-hidden` with presentation status `ok`;
- latch sequence and flip count were `31`, with zero drops;
- the static Home source framebuffer was correct;
- the raw observer saw clock enable, horizontal sync, active samples, and
  substantial nonzero raw pixels in fresh completed frames.

This proves the incident is downstream of the authoritative framebuffer and
is not a software-rendered corrupt image. The current observer proves raw
activity but cannot prove HS/VS/DE ordering, line count, active width, or
raw-to-final phase. It therefore does not locate the exact failing boundary.

## Exact candidate identity

- diagnostic architecture: `raw-scaler-boundary-v1`;
- FPGA implementation commit:
  `dee39545ba8933957a84cb4e005c453234fca5ae`;
- RBF SHA-256:
  `9f7fdd78041bf11638618f51e243157ed33db259081b283f1e90b21738c1192f`;
- metadata SHA-256:
  `b921da728008b1663a0617d4c4752dafe3147cc0e9c6d5b5bd1cc06c26613ebe`;
- signoff report SHA-256:
  `28833829619d20d427b16609e38cc4a376a395af1da1e690f9a473e3b718a56b`;
- latch protocol/capabilities: `5` / `0x03ff`;
- launcher build/source: `0.2.5216` /
  `d903ea217a506eedb5b818f3e15b704b6bad6d8c`;
- Main revision from the installed local-Main transaction:
  `f290719e97f5a3c84efa8e24691b80673b93f23c`;
- host repository at capture:
  `92131d39f1b2311a6fb67fb1d16466d28959e4eb`.

## Integrity manifest

[`2026-08-23-moving-band-corruption-incident-v1.json`](2026-08-23-moving-band-corruption-incident-v1.json)
contains the path, byte count, modification timestamp, and SHA-256 of all 793
retained artifacts, including every extracted frame and the exact local RBF
and metadata. Its ordered artifact-set digest is:

```text
8902bd90e9b84dba1309f830949445ee6253ff8b271fba6c78afea8fba837b4f
```

The manifest declares `preservation_state: "captured-unrecovered"`. Recovery
is deliberately outside this preservation step.

The committed manifest file itself has SHA-256:

```text
606441590d57015d30f2828a260053678f3742dac27fce4ce86dbf75c4f3a5f3
```

## Bounded conclusion

This incident establishes a dynamic physical-output corruption with a correct
internal framebuffer and active raw-scaler evidence. The repeated downward
motion is consistent with a line/frame phase or timing-epoch defect, but this
record does not claim a final root cause or that the black-screen occurrence
is necessarily the same mechanism. A later phase-oriented diagnostic may use
this evidence; no new diagnostic design or device recovery is part of this
incident-preservation commit.
