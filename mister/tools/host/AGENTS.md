# AGENTS.md - mister/tools/host

Root `AGENTS.md` applies.
File authority is documented in `docs/agents/file-authority.md`.

## Ownership

This Rust host tool is the human/operator `mister` binary: SSH/agent transport,
status output, read-only catalog inspection, INI mutation, reboot supervision,
diagnostics, and host metadata tooling. Start with `src/main.rs`;
transport lives in `remote.rs` and `agent_client.rs`.

## Rules

- Keep destructive operations explicit and bounded.
- Permit one-shot recovery reboots only through a closed typed request that
  clears persistent arming, refuses known reboot instability, waits for health,
  and is never automatically replayed.
- The implicit connection bootstrap may transactionally install or upgrade only
  `mister-magik-agent`, its init hook, and its token. It must verify the new
  authenticated version and roll back on failure before the requested command
  runs; no other implicit device mutation is permitted.
- Device communication must remain behind typed `DeviceRequest` or fixed
  operator subcommands; do not add generic shell orchestration callers.
- Preserve direct database-query support; do not assume device `sqlite3`.
- Never weaken reboot-loop cleanup or timeout policy.
- Test parsing, command construction, and safety policy without a device.

## Assurance

Use `$magik-rust-lsp` for Rust navigation and diagnostics. Pre-commit checks
staged formatting; pre-push and CI build and test the runnable host binary and
run full Clippy assurance. Running the tool against the MiSTer requires
first-attempt escalation.
