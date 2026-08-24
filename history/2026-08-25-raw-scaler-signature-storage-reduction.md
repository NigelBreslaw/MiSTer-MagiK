# Raw-scaler signature storage reduction — 2026-08-25

## Exact failed-fit evidence

Committed candidate `d26958b7759ef37a3289aa7e781d63dfbe45833e` completed
fixed-seed Quartus fitting with positive timing but failed the unchanged local
delta gate. Setup slack was `0.306 ns`, below the experimental `0.350 ns`
floor, and its aggregate 234-register delta exceeded the 224-register cap.
The retained delta report SHA-256 is
`8b62a702764a594848c34fa326464fe0c6ef70edef55b705af0e0227a92e882b`.

The worst nine setup paths were production `ascal` counter paths from
`o_hcpt[0]` to `o_vcpt` or `o_vcpt_pre` registers. No disposable-observer
endpoint appeared in the 25 reported setup paths. The next candidate therefore
does not touch production RTL, timing constraints, seed, fitter settings, or
signoff limits. It reduces only observer storage and fanout so the unchanged
fixed-seed placement has less diagnostic pressure.

## Minimal coherent reduction

The ordered signature remains 32 bits and the wrapping source sequence remains
16 bits. The schema-8 command, six response words, flags, CRC, interpretation,
and host reader are unchanged. The HDMI-to-system stable bundle now contains
only `{ordered_signature, frame_sequence}` as 48 bits. A single destination
valid bit is set only when that coherent bundle is captured after the existing
two-stage generation synchronizer and settle cycle.

Before this change, the one-bit valid fact was written as a 16-bit published
flags value in the HDMI domain and another 16-bit word inside the system-domain
snapshot. The RTL rewrite removed 31 nominal storage bits and retained validity
across sequence wrap. Fixed-seed synthesis then proved that Quartus had already
collapsed those constant-zero flag bits: the observer hierarchy fell only from
188 to 187 real registers.

The preserved isolation stage remains the sole consumer of direct `ascal` CE,
RGB, DE, HS, and VS. Source generation, its exact two-stage synchronizer,
read-only command `0x67`, CRC-16/CCITT-FALSE, latch-v5, capabilities `0x03ff`,
and all protected production cones remain unchanged.

## Rejected fit

Committed candidate `bf2590f39a54407475ba98bef8235406e694eab0` recovered
timing to `0.531 ns` setup and `0.243 ns` hold with zero TNS, but failed the
unchanged resource and CDC gates. Aggregate register growth was 253. The fitter
duplicated `source_generation` for routing, leaving the named original without
a path to `generation_meta`; the exact net-delay report therefore contained
only the two completion paths and emitted two new warning 17866 instances.
The retained delta report SHA-256 is
`965ea7382851780001b5c7aa65b5e51154d8dcd4bbe77621241d77ab028fdf79`.
This candidate is rejected and was never installed.

## Rejected candidate 4 and exact CDC identity repair

Committed candidate `282cea91372b774e939fa41e17626deff33aad40`
retained schema 10 and reduced the observer to a 16-bit ordered RGB565
signature. Its fixed-seed fit passed every timing and resource gate: setup
`0.602 ns`, hold `0.249 ns`, zero TNS, and matched-baseline growth of 117 ALMs
and 193 registers with unchanged RAM, DSP, and PLL identity.

It failed only the exact diagnostic CDC identity and warning gates. Quartus
implemented the requested route as
`source_generation~DUPLICATE -> generation_meta`, so the required named
`source_generation -> generation_meta` row was absent. This also added two
warning 17866 instances. The retained delta report is
`build/fpga-local-apple/signoff/quartus-delta-signoff.tsv`, SHA-256
`1f6b9c858c3db8774cf5bf66967b895c060bd6b6ee064d94f7f8899c24085bc1`.
The rejected RBF `cc2c429a…` and metadata `37f86f85…` must never be installed.

The forward candidate preserves the complete candidate-4 datapath, schema,
response, resource reduction, constraints, and protected production cones. It
adds exactly one HDMI-domain `generation_launch` register. Its only data
fanout is `generation_meta`; `source_generation` toggles with the completed
signature and `generation_launch` captures it on the following HDMI edge,
after the source bundle is stable. The exact bounded CDC identity becomes
`generation_launch -> generation_meta`. No duplicate wildcard, warning waiver,
gate relaxation, seed change, or fitter change is permitted.
