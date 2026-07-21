# MiSTer MagiK device agent

The device agent provides authenticated, bounded operations used by the Rust
`mister` operator tool and typed `DeviceRequest` workflows. The host tool may
transactionally bootstrap or upgrade only the agent binary, init hook, and
token; it verifies the authenticated replacement and rolls back before running
the requested operation if bootstrap fails.

Useful attended operator commands include:

```text
mister agent ping
mister agent status
mister agent logs
mister agent timeline
mister agent diagnostics --out build/agent-diagnostics/sample
mister agent magik status
mister agent magik restart-launcher
mister agent magik return-to-launcher
mister agent reboot-wait --timeout 40
```

Maintained automation must not call these through shell wrappers. Agent-facing
delivery, benchmarking, diagnosis, and release acceptance use the typed Rust
state machines. Credentials or authentication failures are reported immediately
with one next action; they are never repaired by printing or replacing secrets.

For physical recovery, delete `/etc/init.d/S00magik-agent` from a mounted SD card.
For boot-loop recovery, follow `docs/device.md` and clear every arming path before
another network attempt.
