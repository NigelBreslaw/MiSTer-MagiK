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

Before this change, the one-bit valid fact occupied a 16-bit published flags
register in the HDMI domain and another 16 bits inside the system-domain
snapshot. Reconstructing the unchanged flags response from one destination bit
removes 31 real registers and associated bundle fanout. Sequence wrap to zero
does not clear validity. The response remains immutable for the entire UIO
transaction because bundle capture and valid-bit update are both blocked while
a command is active.

The preserved isolation stage remains the sole consumer of direct `ascal` CE,
RGB, DE, HS, and VS. Source generation, its exact two-stage synchronizer,
read-only command `0x67`, CRC-16/CCITT-FALSE, latch-v5, capabilities `0x03ff`,
and all protected production cones remain unchanged.
