# Passive FPGA HDMI evidence

The current diagnostic is a deliberately small, read-only recorder for the
physical HDMI FPLL lock. It answers one bounded question after a rare black
return to Menu: did the real HDMI clock PLL remain locked for the lifetime of
the current FPGA configuration?

It does not change the framebuffer latch, scaler, SDRAM, reset, clock control,
PLL reconfiguration, or any video output. It records evidence; it does not
repair the black-screen fault or claim that lock is its cause.

## Hardware contract

UIO command `0x60` returns `hdmi-lock-evidence-v1`, a fixed four-word record:

1. schema;
2. lock flags;
3. saturated lock-loss count;
4. CRC-16/CCITT-FALSE.

The flags report whether lock was seen high, whether two consecutive
synchronized high samples armed the recorder, the current synchronized lock
state, whether lock was ever lost after arming, and counter overflow. A
one-sample high pulse records `seen_high` but does not arm or create a false
loss. Reads snapshot the record atomically and never arm, clear, or otherwise
mutate it.

Commands `0x5D`–`0x5F` belong to the retired schema-4 three-domain observer and
are unsupported by the current FPGA. New device software probes `0x60` first
and falls back to the old records only when `0x60` is explicitly unsupported,
so older qualified RBFs remain diagnosable. A malformed, unknown-schema, or
CRC-invalid `0x60` response is an error and never triggers legacy fallback.

Compatibility is explicit:

| Device agent | FPGA RBF | Result |
| --- | --- | --- |
| new | new lock recorder | v2 lock-evidence JSON from `0x60` |
| new | older schema-4 observer | unchanged v1 JSON after explicit unsupported fallback |
| old | new lock recorder | safe unavailable/unclassified result because `0x5D`–`0x5F` are unsupported |
| old | older schema-4 observer | legacy v1 JSON |

The unavailable/architecture-unknown envelope intentionally retains schema v1
for compatibility. Only a successfully decoded `0x60` record emits the v2
lock-evidence shape.

## Clock-domain boundary

The physical status source is the existing `reconfig_from_pll[16]` signal. The
stock `pll_hdmi` wrapper keeps its redundant lock output terminated; generated
Intel IP is unchanged.

The status has exactly one raw consumer: the first register in a two-stage
`clk_sys` synchronizer. The only diagnostic timing exception is a false path to
that first-stage data pin. The first-to-second-stage settling path remains
timed and must appear in Quartus metastability analysis. All recorder state and
readout logic are in `clk_sys`; there is no wide payload CDC, mailbox, Avalon
observer, HDMI-pixel observer, max-skew constraint, net-delay constraint, block
RAM, or DSP.

## Collection and interpretation

Collect evidence before rebooting or reconfiguring the FPGA:

```text
scripts/agent device diagnostics --out PATH
```

The authenticated device agent uses the existing UIO advisory lock and writes
`fpga-video-diagnostics.json` into the support bundle. Raw UIO is never read by
the SSH fallback. Collection requires the agent transport, `LauncherActive`,
active MagiK latch ownership, and stable owner epoch, launcher state, and latch
ownership across the capture. Otherwise the bundle records the evidence as
unavailable or incoherent rather than issuing an unowned FPGA transaction.
Before each UIO command, the agent lowers IO enable and completes an
acknowledged strobe while it remains disabled. That acknowledgement proves the
disabled state crossed the HPS-to-`clk_sys` synchronizer and reset the FPGA
command parser before the next opcode is raised.

The lock-only classifications are:

- `hdmi_pll_lock_lost`: lock was lost at least once after stable arming;
- `hdmi_pll_locked`: lock is currently high and no loss was retained;
- `hdmi_pll_not_stably_armed`: a high sample was seen but stable arming was not;
- `hdmi_pll_not_seen`: lock has not been sampled high in this configuration.

`hdmi_pll_locked` narrows the next investigation but does not prove visible
video. It cannot distinguish a stopped raster, final registered black pixels,
or a downstream transmitter/display problem. Final-HDMI evidence, if added,
must use a separate versioned contract and pass its own CDC, characterization,
and matched-signoff milestone before deployment.

## Qualification

The fixed-seed-2 local signoff at synthesis commit `840605cf` and
assurance-complete commit `23b5f5d2` passed the unchanged hard timing gates:

- setup slack: 0.474 ns;
- hold slack: 0.247 ns;
- total negative slack: zero;
- unconstrained output paths: 158, equal to the pinned pre-observer build;
- one added calculable synchronizer chain;
- resource delta: 64 ALMs, 25 registers, no block memory, no DSP.

The product ceiling is 800 ALMs and 96 registers. The local build used 64 ALMs;
the independently fitted GitHub build used 105 ALMs. Both are comfortably
inside the intended architecture budget. Any synthesized-source change still
requires fresh empirical signoff. Setup and hold must remain at least 0.20 ns,
TNS zero, slack degradation no more than 0.15 ns, warnings within the pinned
identity, and unconstrained output paths equal to the pre-observer build.

A canonical local signoff set may be installed only to the Dev layout through
the attended rollback-capable experimental FPGA transaction. It is not release
qualified. Production publication still requires the matched GitHub platform
workflow and normal release qualification.

The transaction activates the installed Dev latch RBF only through Main-owned
`load_core` with the exact Dev manifest path. Root `/media/fat/menu.rbf` is the
stock `update_all` artifact; neither that pathname, its compatibility redirect,
nor a `mister_magik_reload_main` process replacement is valid activation
evidence for an experimental RBF.

Device acceptance must exercise the new RBF's v2 record, an older qualified
schema-4 RBF's unchanged v1 fallback, and the SSH unavailable path. Each test
must verify that CRC/semantic failures never fall back and that SSH never reads
raw UIO.

The failed wide-observer attempts and their rejected RBF evidence remain in
[`history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md`](../history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md).
