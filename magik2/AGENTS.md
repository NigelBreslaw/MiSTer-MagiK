# AGENTS.md — MiSTer MagiK Tooling 2.0

`magik2/` is independently owned development tooling. It may not import, wrap,
or invoke `scripts/agent`, `agent-cli`, or `mister/tools/agent`.

- Keep host orchestration in `host/magik2`, native device service code in
  `agent/`, and the disposable consumer experiment in `probe/`.
- A compatible agent is selected by required capabilities, never build or
  version equality. Native payloads are binary framed; never base64 them.
- Device mutations remain explicit attended operations. Keep installation under
  `/media/fat/mister-magik2` and temporary state under `/tmp/mister-magik2`.
- Tool-core changes (`host/magik2/**`, `agent/**`, protocol docs) need a
  dedicated tooling PR. Probe and scenario changes are consumer changes.
- Run focused tests only: `uv run --project magik2/host pytest tests -q` and
  `scripts/cargo test --manifest-path magik2/agent/Cargo.toml`.
