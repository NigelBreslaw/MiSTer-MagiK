# FPGA development

Preserve the last qualified platform. Prove structural, simulation, and formal
properties before synthesis; only fixed-seed Quartus signoff proves area/timing.

Local synthesis uses `QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup` and
`scripts/agent fpga signoff`, never underlying tools. Signoff uses committed
local `main`; freeze the candidate and preserve source, RBF, metadata, and
report hashes together. Preserve completed cache evidence before replacement.
Never seed-sweep, waive timing, add false paths, alter fitter settings, or change
unrelated RTL to rescue failure.

A local pass permits only attended rollback-capable Dev installation. CI must
rebuild the exact platform tuple before release qualification. Physical
output-rate evidence is mandatory for visual claims; acknowledgements alone
are insufficient.

Consult `docs/fpga-development.md` for proof/cost models and cache work, or
`docs/fpga-latch-release.md` for release requirements.
