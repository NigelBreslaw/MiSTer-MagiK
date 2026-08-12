# Seed-1 real HDMI PLL lock signoff evidence

On 2026-08-12, local Apple-container Quartus signoff synthesized the committed
real-HDMI-PLL-lock observer at main `81e55609`. The RBF built successfully but
was rejected by the unchanged delta checker and was not installed.

## Result

- Quartus: 17.0.0 Build 595
- Fitter seed: 1
- Final setup slack: 0.155 ns; required minimum: 0.200 ns
- Final hold slack: 0.243 ns
- Final TNS: 0
- Baseline/final unconstrained output paths: 158/160
- Diagnostic CDC minimum skew/net-delay slack: 4.608/4.003 ns
- Diagnostic synchronizer chains: 399; worst-case MTBF greater than one
  billion years
- Invalid reasons: `setup_slack_below_minimum`,
  `unconstrained_output_paths_added`

The retained local checker record is
`build/fpga-local-apple/signoff/quartus-delta-signoff.tsv`.

## Diagnosis

The 0.155 ns setup path is legacy Menu HDMI logic from the shadow-mask RAM to
`hdmi_osd|osd_en[0]`; no MagiK diagnostic node is on the path. The exact
pre-observer seed-1 build is worse at 0.126 ns, confirming placement sensitivity
rather than a new diagnostic timing cone.

The full unconstrained-path report added in `97b1b04b` identifies two
router-created final rows:

- `emu:emu|act_cnt[7]~DUPLICATE` to `LED[0]`
- `emu:emu|act_cnt[7]~DUPLICATE` to `LED[4]`

The final fitter report states that this register duplicate was inserted for
routability. The pre-observer fitter report contains no such duplicate. The
baseline aggregate difference is exactly two paths; a future full baseline UCP
row comparison remains desirable for exact row-by-row confirmation.

Replacing the added `pll_hdmi.locked` wrapper export with the identical existing
`reconfig_from_pll[16]` status bit did not change either failure. That disproves
the wrapper-export hypothesis and leaves both rejected gates as deterministic
seed-1 fitting outcomes. No timing gate, exception, latch source, or functional
video cone was changed in response.
