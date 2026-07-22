# AGENTS.md - mister/tools/agent

Root `AGENTS.md` applies.
File authority is documented in `docs/agents/file-authority.md`.

## Ownership

This is the small device-side network/boot agent. It owns authenticated command
handling, SD browsing, library snapshots, framebuffer capture/stream proxying,
telemetry, and Linux-specific device operations. Start with `src/main.rs`;
the scanout contract is in `scanout_slots_contract.rs`.

## Rules

- Validate all lengths, paths, and decoded sizes before allocation or I/O.
- Keep non-Linux tests functional.
- Never expose credentials or add unauthenticated commands.
- The steady-state framebuffer stream proxies producer frames; it must not poll
  `/dev/fb0`.
- OS access stays isolated from request validation where practical.

## Checks

```bash
scripts/agent check
scripts/agent verify
```

Building/deploying the ARM agent and all device communication require
first-attempt escalation.
