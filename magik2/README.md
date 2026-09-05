# MiSTer MagiK Tooling 2.0

This is a separately owned replacement for the development loop. It does not
modify the production MiSTer MagiK application, Main, or FPGA platform.

The public entrypoint is `scripts/magik2`:

```text
scripts/magik2 deploy
scripts/magik2 check smoke
scripts/magik2 check motion --profile
scripts/magik2 watch
scripts/magik2 status
scripts/magik2 stop
```

The host uses `MISTER_IP`; `MISTER_MAGIK2_TOKEN` authenticates native requests.
The native service defaults to TCP port 7500. SSH is reserved for a future,
automatic bootstrap/repair adapter and is not called by this implementation.

## Current implementation

The initial independently testable contract is in place: bounded authenticated
request envelopes, length-delimited binary payloads, capability negotiation,
atomic hash-checked upload publication, result bundles, and a stdlib native
agent. The command entrypoint intentionally reports an actionable error until a
device bootstrap adapter and the probe artifact are supplied; it never falls
back to the legacy agent.

The wire contract is deliberately small. JSON headers are capped at 64 KiB;
bulk bytes follow a fixed 64-bit big-endian length and are never encoded into a
header. Optional response fields are ignored by the host.
