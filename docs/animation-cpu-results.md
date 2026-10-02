# Animation CPU campaign results

The campaign uses the merged reporting contract and the complete 45-second
Root → Consoles → Nintendo → SNES hub → Games → Root route. Every route below
used a fresh native Dev lease and completed all three repetitions. CPU is
process CPU per confirmed moving presentation, including background app threads;
it is not CPU utilization or isolated renderer CPU. Emulator runs establish
pixel correctness only. The 60 FPS / zero-drop target remains unmet.

## Proven opaque projected compose — kept (`66e082f74`)

The generic kernel checks alpha through NEON-to-ARM transfers inside its loop.
The replacement uses the existing canonical eight-source-row opacity contract
to partition the projected output into generic rounded caps and an opaque
interior. Both bilinear inputs stay inside that interior. Geometry, sampling,
quantisation and presentation timing are unchanged.

| Measurement | Parent | Opaque candidate |
| --- | --- | --- |
| Compose, four native samples (ns/pixel) | 64.780–65.001 | 52.921–53.047 |
| Complete-route drops | 36 / 39 / 36 | 21 / 26 / 23 |
| Mean complete-route drops | 37.00 | 23.33 |
| Moving CPU per presentation (ms), three runs | 20.431 / 20.488 / 20.500 | 19.722 / 19.887 / 19.752 |
| Mean moving CPU per presentation (ms) | 20.473 | 19.787 |

Compose was approximately 18.5% faster. Route drops fell about 37%, and moving
CPU per presentation fell 3.4%. Card-workload drops were 27/29/27 before and
13/18/14 after. Slint and system-transition drops remained: 9/10/9 before and
8/8/9 after. These three-run comparisons describe the measured workload, not a
guarantee under other device load.

Validation: 177 portable scene tests passed (one ignored), focused Clippy passed,
and ARM parity covered 40,000 projected profiles plus exhaustive interpolation,
blend and final-over cases. The native fixture also compared the entire output
before timing. Generic and opaque kernel samples came from the same executable,
with production compiler flags; their order was reversed on the second pair.

Parent Dev SHA256: `9fc4e1c06b67a5e1c75bed032685d05ae775e2f0532584911f061198c85cc19d`.
Candidate Dev SHA256: `730a2c546f3efbcd62d964471810314dbb066d91ba2b20551f78212bfe927217`.
Route evidence: `build/magik-results/20261002T185943Z-ec4a3edada6a` and
`build/magik-results/20261002T190541Z-f41e9f862f91`.
Raw evidence stays ignored; it is not shipped with source.

## Four-column transpose — rejected

A standalone trial composed four independent projected columns, transposed the
four-row NEON results, and stored contiguous four-pixel row runs. It retained the
opaque loop for rounded caps and unmatched tails. Ten thousand randomized ARM
profiles with independently varying source starts/strides matched generic pixels.

On the same native fixture, block compose measured 55.579–55.700 ns/pixel versus
52.922–53.192 for the opaque column loop. Reversing run order confirmed the
regression, approximately 5%. Contiguous stores alone did not pay for the extra
live column state and transpose work in this implementation. No production
integration was made, so another full-route deployment was unnecessary.

Trial source, executable and raw results are retained locally in ignored
`outputs/animation-cpu-round-2/block-*` and `compose-block*`. This rejects the
measured four-column implementation; an eight-column or different layout remains
unmeasured. It does not establish that every row-store approach is slower.

## Flat opaque compose — production change discarded

Four native fixture samples measured 34.805–35.049 ns/pixel for the opacity trial
versus 50.850–50.892 for generic flat compose (about 31% faster). The integrated
trial also passed 4,000 randomized flat-strip cases and the existing ARM parity
suite. Its three complete routes recorded 25/23/19 drops versus 21/26/23 for its
opaque-projected parent. Mean moving CPU was 19.806 versus 19.787 ms/presentation;
mean summed band wall time was 18.788 versus 18.866 ms/card frame. These small
route differences do not establish an improvement.

The fast path was removed instead of retaining production complexity for an
unproven benefit on the target route. The `card-flat-generic` native fixture and
independent projected-reference parity checks remain for future flat-kernel
work. Trial Dev SHA256 was
`466d96ee1e0843e4184373d5834cf39d47a6ce487db71e92374559b28b954ba1`;
route evidence is `build/magik-results/20261002T191612Z-bd8dc35a2a54`.

Further campaign items remain in `animation-cpu-campaign.md`.
