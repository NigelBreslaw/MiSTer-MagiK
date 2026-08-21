# Outbound Runtime upload qualification — 2026-08-22

## Decision

Runtime binary staging now travels from the host to the MiSTer over the
existing authenticated agent TCP connection. The Mac no longer creates a
one-shot HTTP listener, so runtime delivery has no host-side inbound accept
site. The coherent deployment transaction remains authoritative for the lock,
Main suspend/resume, manifest upload, independent SHA-256 verification, atomic
activation, rollback, and launcher health proof.

Telemetry and framebuffer analytics were not moved. They already use bounded,
authenticated agent streams; HTTP framing would add no useful transport
property to either continuous stream.

## Implementation checklist

- [x] Strict two-field `runtime-upload-v1` request contract.
- [x] 128 MiB payload limit and fixed 64 KiB copy buffer.
- [x] Canonical Dev lock and `.upload` destination derived on device.
- [x] Exact EOF, lowercase SHA-256, file sync, atomic rename, and directory sync.
- [x] No receiver authority to suspend, swap, chmod, or update the manifest.
- [x] Agent version 27 capability negotiation.
- [x] Host-to-device stream with write-side shutdown and response evidence.
- [x] Exact size/hash reconciliation before one SFTP fallback.
- [x] Mac HTTP listener and device curl route removed.
- [x] Host source guard rejects future TCP listener and UDP bind sites.

## Device results

Both runs used the canonical `scripts/agent deliver` workflow and passed the
coherent runtime smoke check. The small artifact-size difference is from the
transport implementation itself.

| Measure | HTTP baseline `c4b58464e` | Agent stream `53c11ad1a` | Change |
|---|---:|---:|---:|
| Runtime bytes | 31,757,092 | 31,757,128 | +36 bytes |
| Binary transfer | 11,377 ms | 6,388 ms | -4,989 ms (-43.9%) |
| Device receive | not reported | 6,374 ms | 4,982,291 B/s |
| Coherent transaction | 13,629 ms | 8,663 ms | -4,966 ms (-36.4%) |
| Runtime smoke | 3,839 ms | 3,307 ms | passed |
| Binary transport | `http` | `agent-stream` | no fallback |
| Host inbound listener sites | 1 | 0 | eliminated |

The acceptance bound allowed the agent stream to be at most the larger of
10% or 100 ms slower than HTTP. It was 43.9% faster, used no fallback, and the
post-activation smoke check passed.
