# Attended PAL CRT probes — 2026-07-24

This record captures the first bounded attended probe run for the PAL-only
motion fault on the physical CRT. Generated reports and private video remain
outside Git.

## Provenance

- Probe implementation: `5d281b28c13c93d78b50e23d05047c45772ef2f7`
- Probe post-observation correction:
  `01c78373190f9e7d2dc3fba89e85c986d8699e97`
- Delivered application source revision:
  `01c78373190f9e7d2dc3fba89e85c986d8699e97`
- Local delivered application binary SHA-256:
  `26ebba985083adcd4df1799fb684989b33ebe39864d2a777cba6d4bdf9696e82`
- Main source revision:
  `d359f79fad3682cb0942060163caedd0b2ccc02e`
- Local Main binary SHA-256:
  `8b94ab4ade27e415b6bd2450210a33c863b15f9beddc93df61c2a1ce76fdbdc5`
- Local delivery-input latch RBF SHA-256:
  `087f372680aaccd192208c17d079858d79408039ad4b3a5faea4b3f7ba73e7a5`
- RBF source revision: `3c3634c0105d78f27aeba66b38966c50dbc42c9b`
- RBF builder revision: `a8965c804660449f1bcef5e770fa7307f71db9f3`
- Scanout module SHA-256:
  `bbc0f3a86e31b534ace640f504b3c5a76411aeef957a6aaf8a03c4121ac19dc1`

The binary and RBF hashes above identify the clean local inputs used by the
delivery harness. The typed device status interfaces did not provide file
readback hashes, so this record does not mislabel those local hashes as device
readback.

## Reproduction

All probes used the existing `crt-576p50` mode and restored the supervised
launcher afterward:

```text
mister crt probe --attended --pattern fixed-a --seconds 20 --out DIR
mister crt probe --attended --pattern identical-flip --seconds 20 --out DIR
mister crt probe --attended --pattern motion --seconds 20 --out DIR
mister crt probe --attended --pattern full-ab --seconds 20 --out DIR
```

The operator had already established that both 50 Hz modes exhibit the live
fault and both 60 Hz modes are clean. USB Video does not reproduce the physical
CRT fault.

## Results

### `fixed-a`

- Manual CRT observation: stable static grid.
- Machine result: 20.001 seconds, one post/flip, zero drops, zero cadence
  misses, zero active-buffer writes, zero pending-buffer writes.
- Manifest SHA-256:
  `bc9e8bd869de2c012f56b0e5c44952f1f17bdbabbb7e63d94003c614fc9901bc`

### `identical-flip`

- Manual CRT observation: stable while identical slot contents alternated.
- Machine result before the diagnostic stopped: 470 posts/flips, zero drops,
  zero active-buffer writes, and zero pending-buffer writes.
- The stop was a probe fault: the first status read after a post could still
  describe the previous settled sequence. Commit `01c78373` made the probe wait
  for the posted sequence before accepting settlement.

### `motion`

- Manual CRT observation: clearly unstable. The moving ruler showed the
  lower-region horizontal displacement/ghost seen during normal PAL motion.
- Corrected machine result: 20.006 seconds, 1,010 posts/flips, zero drops, zero
  cadence misses, zero active-buffer writes, zero pending-buffer writes, and a
  matching final active sequence.
- Manifest SHA-256:
  `41a878c63aa4fee11cb326e729f43490950139008357d65b0c719809995b7ffd`
- Phone recording `IMG_4530.MOV` SHA-256:
  `f080ca4ce467534197a45b6ab92b18289bf9ce7ad05cc2986552af0679f71492`
- Native USB Video recording SHA-256:
  `c2087ca87872798a4ce1480eb41adaaf8c105d207d4c9b377064cb124928059f`
- The USB recording did not expose the physical CRT symptom.

### `full-ab`

- Machine result: 20.006 seconds, only two initial buffer writes, 1,010
  posts/flips, zero drops, zero cadence misses, zero active-buffer writes, and
  zero pending-buffer writes.
- Manifest SHA-256:
  `db5722a5ec994d066660c69064db383d1952d1616b739aedc8688f1322659624`
- Phone recording `IMG_4531.MOV` SHA-256:
  `bc4e6237865fa7df16dadcdfaf4a8b5c826b8dd806032447ce8b8517c67731d3`
- Source frames 3, 9, 15, 21, and 27 contain a split A/B grid. That exact
  six-camera-frame interval is the 59.94 Hz camera beating against the
  intentional 50 Hz A/B transition. It is camera rolling-shutter evidence and
  cannot prove or disprove a hardware base-switch tear.

## Conclusion

The reproducible fault requires motion, is visible on the physical analog CRT,
is absent from USB Video, affects both 50 Hz modes, and is absent from both
60 Hz modes. Application publication counters, latch counters, buffer
ownership, and rolling cadence remain clean during the failure.

This rules out the earlier 47.5 fps interpretation and does not support a
speculative software pacing correction. The next investigation target is the
50 Hz relationship among the protocol-v2 pending-base apply event, scaler fetch
frame boundary, `hdmi_vbl`, and the VGA/Direct Video raster. Native VGA blanking
must not replace `hdmi_vbl` without first proving the VGA route lies outside the
same scaler/fetch boundary.
