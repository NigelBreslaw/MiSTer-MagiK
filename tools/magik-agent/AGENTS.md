# AGENTS.md - tools/magik-agent

Root `AGENTS.md` applies.

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
cargo test --manifest-path tools/magik-agent/Cargo.toml
cargo clippy --manifest-path tools/magik-agent/Cargo.toml --all-targets -- -D warnings
scripts/validate paths tools/magik-agent
```

Building/deploying the ARM agent and all device communication require
first-attempt escalation.
