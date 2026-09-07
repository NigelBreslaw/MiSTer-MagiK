# Remembered MiSTer access

Select a board once with `scripts/magik device select ADDRESS`. Explicitly supplied
`MISTER_USER` and `MISTER_PASS` bootstrap inputs are imported into macOS Keychain
through Security.framework. Do not put a password in a shell command, repository
configuration, MCP configuration, or result bundle. Other worktrees use the same
Keychain item, associated with the board Ethernet MAC and SSH username.

Non-secret selection lives in `device.json` beneath the existing shared user state
root (`MISTER_MAGIK2_STATE`, otherwise the XDG state directory). Native tokens stay
in permission-restricted files, keyed by board identity. An existing address token
is migrated only after identifying the board at that address. Atomic writes use
unique temporary files so concurrent worktrees do not share a temporary filename.

Ordinary commands, Python scenarios, benchmarks and framebuffer MCP use this
selection automatically. `MISTER_IP` remains an explicit address override. It cannot
silently select a different board; use `device select` to change the selection.

Discovery tries the remembered address, then bounded hostname and neighbour
queries and private directly attached network probes. The search has an eight-second
budget and at most 32 concurrent probes. Large attached networks are limited to the
host's local /24, with at most 512 candidate addresses overall. No public network
is scanned. A sole board can be selected automatically; multiple boards require
explicit selection. A remembered identity always takes precedence over other boards.

The native `identify` operation is read-only and unauthenticated: it exposes only
the board MAC. Status also includes `device_identity`, independently of the
informational service build `identity`. Control remains authenticated. Older or
absent services can be identified through the fixed SSH bootstrap adapter; identity
support alone does not force replacement of a compatible service.

Offline, ambiguous selection, network access denial, SSH authentication failure and
Keychain failure are reported separately. No plaintext password fallback or
credential retry loop is provided. Discovery does not reboot or deploy an app.

## Acceptance evidence (7 September 2026)

Two successful read-only checks used board `e2:b1:0a:86:84:3c` at
`192.168.1.117`, without `MISTER_IP`:

- Initial `device select`: 748 ms, imported the already supplied login through
  Keychain and saved the verified identity. Result `20260907T091843Z-6246c8116d5a`.
- `status` with the remembered address deliberately changed to `192.168.1.254`:
  5,367 ms, rediscovered the same identity, retrieved authentication from Keychain
  and retained the compatible installed service. Result `20260907T091914Z-6597b46817d4`.

Before these succeeded, the stdin setup wrapper received EOF and submitted an
empty password once; that authentication failure did not modify device state.
The wrapper was corrected to use a non-echoing stdin channel. No app, service
replacement, reboot or DHCP change was performed. The selected address is restored
to the verified current address. Raw result bundles remain local and ignored.
