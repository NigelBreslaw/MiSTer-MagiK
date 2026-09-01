# AGENTS.md - FPGA development

FPGA work must fail closed and preserve the last qualified platform.

- Prove cheap structural, simulation, and formal properties before Quartus.
- Simulation and formal prove behavior, not QoR. Use structural synthesis only
  to reject candidates; only fixed-seed Quartus signoff proves area and timing.
- Do not infer FPGA cost from RTL register count. Compare mapped cells, mux
  widths, and fanout with the prior candidate.
- Use only `QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup` and
  `scripts/agent fpga signoff` for local synthesis. Never invoke Quartus, its
  installer, or underlying build scripts directly.
- Signoff builds committed local `main`, not an arbitrary worktree. Freeze the
  exact candidate before the expensive build and preserve source, RBF,
  metadata, and report hashes together.
- Before replacing a completed patched cache, preserve its RBF, metadata,
  reports, and delta result under its commit.
- Never seed-sweep, waive timing, add false paths, alter fitter settings, or
  change unrelated RTL to rescue a failing candidate.
- A local pass permits only an attended rollback-capable Dev install. CI must
  rebuild the exact platform tuple before release qualification.
- Physical output-rate evidence is required for visual claims; framebuffer or
  protocol acknowledgement is insufficient.

Consult `docs/fpga-development.md` only for proof-model or Quartus-cache work.
Consult the relevant section of `docs/fpga-latch-release.md` only while
qualifying a release. Hooks, typed FPGA signoff, CI, and attended qualification
own their respective gates.
