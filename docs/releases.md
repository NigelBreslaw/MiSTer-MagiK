# Releases

Release artifacts are assembled by host-only packaging and release-data tools
under `scripts/release/`. Runtime/platform qualification remains in Rust.

For a release candidate:

```text
scripts/agent task begin
scripts/agent check
scripts/agent verify
scripts/agent commit -m "Prepare release"
scripts/agent deliver
scripts/agent release qualify
```

Platform delivery may remain `external_required` until the exact commit is
published on `main`; rerun `deliver` after CI publishes the matching artifact.
Delivery never pushes automatically.

The attended qualification is fixed and flag-free. It checks runtime, catalog,
input/handoff/return, display, recovery capability, and restoration. Rollback is
the typed `mister mode stock` operator command. Packaging output, credentials,
caches, and private fixtures must not be staged.

CRT qualification additionally records a
`mister-magik-crt-qualification-v1` evidence document. It binds the exact app
and Main revisions, RBF and platform-contract hashes, protocol-v3 hash, timed
trial measurements and counters, CRT/HDMI switching, launcher/OSD/input/game
lifecycle checks, HDMI resolution regression matrix, cleanup, limitations, and
rollback. Until that attended document passes
`scripts/checks/verify-crt-qualification-evidence.py`, CRT remains implemented
but not hardware-qualified.
