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

## Scaler-safe ownership-release experiment

Commit `3913a1c14d5d986b9d4110190ef9abceb3b19fa6` retained the early
`hdmi_vbl` route apply but delayed `active_seq`, `flip_count`, and old-slot
ownership release until blanking ended. Commit
`757b4ec0f14e215c39818ef148fef3e11f212e14` corrected one test expectation.
The experiment tested whether software was rewriting the old slot after the
latch advertised a flip but before `ascal` captured the new base.

- Qualified platform workflow: `30089626684`
- Delivered source revision:
  `8fbdfd8ebad7db49dd80459fa04bc189e62e70c8`
- FPGA component ID:
  `50da9ccca1a4581db6959d3e13d7e0d05554e2bce24ba8fe7e9fe3ac05aa5952`
- Patched RBF SHA-256:
  `431f82b08b203f0c40807b1ac3091134c4d8a412e32fcc4d5f3eac0e744099a3`
- Quartus delta: positive setup and hold slack, zero TNS, identical warning
  count, and one additional calculable synchronizer chain.

The attended `motion` probe remained visibly unstable with the same lower-band
horizontal displacement/ghost. Machine telemetry reported 20.006 seconds,
1,009 posts/flips, zero drops, zero unsafe active writes, zero pending writes,
a matching final active sequence, and one isolated cadence miss. Report hashes:

- Manifest:
  `85a144f7f2a62f3fa88400766714afee979311684e08a481200c4a18d912f88b`
- Status:
  `20e4c9c9d1fc98c2ae3e32cacd3f69a4923f4ac554856d7495455dfa62c8715f`
- Log:
  `93fb6cbb6ff73ee634f13ca6b25b95d02b66ebfb86e9d6a988e76550ebdc8b81`

The attended `slow-ab` probe changed between the 24-pixel-offset A/B grids once
per second. No physical CRT corruption was observed. Machine telemetry reported
20 posts/flips, zero drops, zero unsafe active writes, zero pending writes, and
a matching final active sequence. Its cadence-miss count reflects the
intentional one-second transition interval rather than missed 50 Hz presents.
Report hashes:

- Manifest:
  `c3c142c5418241e23a8920e5be098fc981f83e1fff27ab1eb8ee5e666f66178e`
- Status:
  `6121f3997e42b2f61bd84a66b669bd52caba2522a8653843de00e4a3956fbc9a`
- Log:
  `89a0fbed7bf5fd01860487913b84305c69a92cb773b16abf68e1c02d314fef23`

The experiment is disproven: delaying old-slot ownership release through the
entire blanking interval does not affect the continuous-motion fault. Isolated
base changes are not visibly corrupt; sustained frame-by-frame change is
required. The next candidate must therefore distinguish scaler fetch/pipeline
history under continuous alternation from an analog CRT-only 50 Hz temporal or
sync effect.

## Human-readable transition isolation

Later probes replaced the difficult-to-score A/B grids with a bright moving
ruler and then with high-contrast preloaded bars. These results supersede the
earlier conclusion that isolated base changes are clean.

Provenance for the decisive high-contrast run:

- Delivered application source revision:
  `dffa74d2dd5b81e5ca19229f3241c853abdf0b14`
- Local delivered application binary SHA-256:
  `e80e1bc8ba9d37c69379443e00dde3eccdf978eabbe72cf4d2b3da886c17dfa9`
- Main source revision:
  `d359f79fad3682cb0942060163caedd0b2ccc02e`
- Local Main binary SHA-256:
  `8b94ab4ade27e415b6bd2450210a33c863b15f9beddc93df61c2a1ce76fdbdc5`
- Local delivery-input latch RBF SHA-256:
  `b1a7e0c804b6b6f38b52b3dc425d59ceca906bcbdcd041a0ccfb50dbae7663ff`
- RBF source revision:
  `3c3634c0105d78f27aeba66b38966c50dbc42c9b`

The moving ruler remained visibly faulty when held for two rasters (25.2
updates/s) and three rasters (16.8 updates/s). A ruler moving only once per
second still intermittently left its lower section at the old position. All
three runs had matching final routes, zero latch drops, zero active-buffer
writes, and zero pending-buffer writes. Their manifest hashes were:

- `motion-hold2`:
  `a450bc0fb4ea6527b55114eb7acfbc665e2adc84774f0c8d3dce66b2a36a89f9`
- `motion-hold3`:
  `814629f3f71a8f39f0e4a6c4e6d3b1f5a98591cefdaccecef6cd558b3408af07`
- `motion-slow`:
  `659dfcc614f8e45549548b7448c948075886c05dab43798a1f67786bca77daee`
- repeated `motion-slow`:
  `bf491f3ff80d7643fde9f3050ec0181d9e594ce29c64e1effd744d221976082f`

The decisive `preloaded-bars-slow` probe wrote a cyan-left frame and a
magenta-right frame once before observation, then switched only the framebuffer
base once per second. It performed no framebuffer writes during observation.
The operator clearly observed the bottom of the old bar being left behind
during a transition. Machine telemetry reported two writes, 20 posts/flips,
zero drops, zero active-buffer writes, zero pending-buffer writes, no pending
final transaction, and a matching final active route.

- Manifest SHA-256:
  `9ef6d2062bb88fc68e6b41dc85b17bd73b5719df7c775aad94e856adaddd9119`
- Status SHA-256:
  `1d924354b327184b83c1d93d97273a9ee181a8d647e69fc1dadcafbadf6a2ab5`
- Log SHA-256:
  `34f4c1ebece5e180c126d87a3dfe55cdffa5991168867c40e5bcb183f0f7eb7f`

This proves that the visible PAL fault is a mixed-base scanout transition. It
does not require rendering, recent framebuffer writes, full-rate alternation,
or a software cadence miss. The VGA framebuffer route consumes the scaler
output, so native VGA sync is not an independent fetch boundary. The remaining
fault boundary is inside the scaler path: `LFB_BASE` changes in `clk_sys`, while
`ascal` copies and consumes the base in its Avalon fetch domain at scaler VS
falling. The next FPGA experiment must make that fetch-domain base selection
atomic and acknowledge completion only after that event.

## Correction: the preloaded transition probe does not localize the fault

The preceding mixed-base conclusion was disproven by a clearer photograph and
must not be used as the basis for a production FPGA change.

Experimental commit `edb03e2e40824601dcf80c032a9d1c9501f768c6` staged a
pending base for a complete raster before ownership completion. Fast FPGA
simulation, lint, structural integration, and coverage passed in workflow
`30093833712`. Non-publishing platform workflow `30093926589` successfully
built matched stock/patched RBFs, passed Quartus delta verification, compacted
the FPGA component, and assembled the candidate.

Three repetitions of `preloaded-bars-slow` on that candidate retained clean
machine telemetry: two initial writes, 20 posts/flips, zero drops, zero
active-buffer writes, zero pending-buffer writes, no final pending
transaction, and a matching final active route. Manifest hashes:

- `216d4c2d956568632013abf388d067ea28fe3457f9041fb3800a97ff2830517d`
- `fad483c660920d72b9e1a916b8f5f2fdf1379b836cc594f1b8f7ceae972022b7`
- `21b5e58a2cda40affa5cc57a782ebc50d0dc288a9f45979a25a85fa34efd4b2d`

The operator still observed the old bar at the bottom, although the retained
amount appeared smaller. Photograph `Screenshot 2026-07-24 at 16.07.46.png`
(SHA-256
`08f944e24e4fac51e730073e28a9eb1f7db05ff53a9817cfbd2bba1b8a221c08`)
shows one horizontal time boundary across both preloaded bars. The operator
then clarified that the display is an OLED television: MiSTer VGA feeds a
Morph4K analog bridge, whose HDMI output feeds the OLED. There is no CRT
phosphor in this path, so the earlier phosphor explanation was wrong.

The image and attended observation still do not identify which component
created the boundary. Candidates include MiSTer's 50 Hz VGA clock/sync/analog
output, Morph4K analog capture or frame conversion, OLED 50 Hz processing, and
any topology difference between the USB capture and OLED paths. The USB Video
tap point must be established before its clean result can localize the fault.
The apparent reduction under the experiment is not acceptance evidence.

The high-contrast probe therefore disproves neither a digital/analog timing
fault nor a downstream conversion fault. It does prove that the FPGA preroll
candidate failed acceptance. That experimental timing is not promoted and was
removed from the active branch by non-destructive revert `10320431`.
