# Releases

Release artifacts are assembled by host-only packaging and release-data tools
under `scripts/release/`. Runtime/platform qualification remains in Rust.

For a release candidate:

```text
git add -- PATH...
git commit -m "Prepare release"
git push
scripts/agent deliver
scripts/agent release qualify
```

The pre-push hook and CI must pass for the exact release commit before delivery
and attended qualification.
Development delivery builds the app from its clean local commit and never
pushes automatically. Main, the scanout kernel module, and the latch RBF come
from the latest qualified GitHub platform release, with its verified
tag-addressed archive reused across deliveries.

The platform workflow preserves completed stock and patched Quartus synthesis
under a synthesis-only content identity before qualification begins. If a
validator fails and its logic is subsequently fixed, the next run may restore
those identical RBF outputs, but it always reruns the complete FPGA validation
and creates fresh component provenance before assembly or publication.

The attended qualification is fixed and flag-free. It checks runtime, catalog,
input/handoff/return, display, recovery capability, and restoration. Rollback is
the typed `scripts/agent device mode set stock --attended` operator command. Packaging output, credentials,
caches, and private fixtures must not be staged.

CRT qualification additionally records a
`mister-magik-crt-qualification-v3` evidence document. It binds the exact app,
Main, and Menu revisions; kernel, RBF, platform-contract, platform-manifest,
protocol, and FPGA-component hashes; the local alpha device journey; bounded
RGB565 publication trial; resolved standard mode; and external analyzer
measurements for clock, totals, porches, sync widths, polarity, and rates. It
also records launcher/OSD/input, native-core-timing handoff, game lifecycle,
HDMI regression, cleanup, limitations, and rollback. Until that attended
real-CRT document passes
`scripts/checks/verify-crt-qualification-evidence.py`, CRT remains implemented
but not hardware-qualified.

Retained protocol-v2 RBF or CRT evidence is rollback-only and must be verified
with the explicit `--historical-v2` option. It cannot qualify a new platform.
