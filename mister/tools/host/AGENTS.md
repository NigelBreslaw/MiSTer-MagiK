# AGENTS.md - mister/tools/host

Root `AGENTS.md` applies.
File authority is documented in `docs/agents/file-authority.md`.

## Ownership

This Rust host tool backs `scripts/mister`: SSH/agent transport, deployment,
status/doctor output, read-only catalog inspection, INI mutation, reboot
supervision, diagnostics, and host metadata tooling. Start with `src/main.rs`;
transport lives in `remote.rs` and `agent_client.rs`.

## Rules

- Keep destructive operations explicit and bounded.
- The implicit connection bootstrap may transactionally install or upgrade only
  `mister-magik-agent`, its init hook, and its token. It must verify the new
  authenticated version and roll back on failure before the requested command
  runs; no other implicit device mutation is permitted.
- Device communication must remain behind `scripts/mister`.
- Preserve direct database-query support; do not assume device `sqlite3`.
- Never weaken reboot-loop cleanup or timeout policy.
- Test parsing, command construction, and safety policy without a device.

## Checks

```bash
scripts/agent check --paths mister/tools/host
scripts/agent verify --paths mister/tools/host
scripts/agent verify --paths mister/tools/host
```

These checks are host-only. Running the tool against the MiSTer requires
first-attempt escalation.
