# AGENTS.md - device agent

This is the authenticated device-side network and boot agent. Start with
`src/main.rs`; scanout ABI handling starts in `scanout_slots_contract.rs`.

- Validate lengths, paths, and decoded sizes before allocation or I/O.
- Keep non-Linux tests functional and OS access isolated from validation.
- Never expose credentials or add unauthenticated commands.
- Proxy steady-state framebuffer streams from the producer; never poll
  `/dev/fb0`.
- Device communication and ARM deployment require first-attempt escalation.

Use `$magik-rust-lsp` for edit-time diagnostics; hooks and native Linux CI own
automated assurance.
