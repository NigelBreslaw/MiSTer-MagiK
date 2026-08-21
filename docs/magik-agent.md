# MiSTer MagiK device agent

The device agent provides authenticated, bounded operations used by the Rust
`scripts/agent device` operator commands and closed workflow services. The host tool may
transactionally bootstrap or upgrade only the agent binary, init hook, and
token; it verifies the authenticated replacement and rolls back before running
the requested operation if bootstrap fails.

Useful attended operator commands include:

```text
scripts/agent device status
scripts/agent device logs
scripts/agent device events
scripts/agent device diagnostics --out build/agent-diagnostics/sample
scripts/agent device launcher status
scripts/agent device launcher restart --attended
scripts/agent device launcher return-to-launcher --attended
scripts/agent device reboot --attended
```

Maintained automation must not call these through shell wrappers. Agent-facing
delivery, benchmarking, diagnosis, and release acceptance use the typed Rust
state machines. Credentials or authentication failures are reported immediately
with one next action; they are never repaired by printing or replacing secrets.

Runtime delivery uses the internal `runtime-upload-v1` capability. Its request
header declares only `payload_bytes` and a canonical lowercase `sha256`,
followed by the raw bytes on the same authenticated TCP connection. The agent
bounds the payload at 128 MiB, owns at most a 64 KiB copy buffer, requires the
Dev deploy lock, and atomically stages only the canonical `.upload` path. It
does not suspend, activate, chmod, or update the platform manifest; the
coherent host transaction retains those responsibilities. Continuous device
telemetry and framebuffer analytics keep their existing authenticated agent
stream protocols and do not use this upload command.

For physical recovery, delete `/etc/init.d/S00magik-agent` from a mounted SD card.
For boot-loop recovery, follow `docs/device.md` and clear every arming path before
another network attempt.
