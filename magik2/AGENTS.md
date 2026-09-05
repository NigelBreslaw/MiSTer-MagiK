# AGENTS.md — MiSTer MagiK Tooling 2.0

`magik2/` is independently owned development tooling. It may not import, wrap,
or invoke `scripts/agent`, `agent-cli`, or `mister/tools/agent`.

- Keep host orchestration in `host/magik2`, native device service code in
  `agent/`, and Mini-MagiK in `probe/`. Shared application-side observation belongs in
  `crates/tooling-support`; the real application remains in `apps/mister`.
- A compatible agent is selected by required capabilities, never build or
  version equality. Native payloads are binary framed; never base64 them.
- Use typed `scripts/magik2` operations for device work, with first-attempt
  sandbox escalation. An authorized operation includes automatic bootstrap,
  missing-capability installation, and resumption; do not request separate
  user confirmation for those steps.
- SSH is explicitly approved inside the fixed-purpose Python bootstrap/repair
  adapter when the native service is absent/unreachable or cannot recover
  natively. Use the configured device and existing SSH authentication inputs;
  load the SSH library only when needed. Do not expose arbitrary remote shell
  commands. Normal traffic and reachable-service upgrades use native transport.
- Keep installation under `/media/fat/mister-magik2` and temporary state under
  `/tmp/mister-magik2`. Dirty-worktree experiment delivery is supported; legacy
  clean-commit, platform qualification, and rollback gates do not apply here.
  Preserve the real app/platform and use Main's handoff for probe lifecycle.
- Tool-core changes (`host/magik2/**`, `agent/**`, protocol docs) need a
  dedicated tooling PR. Probe and scenario changes are consumer changes.
- Run focused tests only: `uv run --project magik2/host pytest tests -q` and
  `scripts/cargo test --manifest-path magik2/agent/Cargo.toml`.
