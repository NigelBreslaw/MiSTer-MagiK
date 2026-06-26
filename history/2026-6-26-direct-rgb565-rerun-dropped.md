# Direct RGB565 Decode Rerun Dropped

Date: 2026-06-26

## Experiment

Recreated the v2 pixel direct-decode experiment after the first dropped attempt
looked suspicious. The rerun split the synthetic pack benchmark into scratch,
zeroed `Vec<u16>`, uninitialized `Vec<u16>`, and uninitialized `Arc<[u16]>`
decode modes, then tried the largest plausible production-shaped variant:
decode v2 pixel payloads directly into `Arc<[u16]>` with
`Arc::new_uninit_slice` on little-endian targets.

## Results

Pack bytes did not change because the archive format was unchanged:

```text
arcade bytes=24529459 entries=838 raw_bytes=133720960 ratio=0.183438
saturn bytes=9067049 entries=116 raw_bytes=17817600 ratio=0.508882
neogeo bytes=4973975 entries=176 raw_bytes=25295360 ratio=0.196636
```

Same-session full arcade pack decode, 10 iterations over all entries:

```text
scratch     rows=8380 decode_cpu_p99_us=1890 raw565_parse_cpu_p99_us=1312 total_p99_us=3017 total_max_us=13489
arc_uninit  rows=8380 decode_cpu_p99_us=2933 raw565_parse_cpu_p99_us=0    total_p99_us=3006 total_max_us=9396
```

The direct `Arc<[u16]>` path eliminated the parse bucket, but shifted that work
into decode. Total p99 was effectively flat.

First-preview worker samples for the selected `1941` preview stayed valid and
used `index_pread`, but wall time remained noisy:

```text
ARC-01 total_us=1472  read_us=281 decode_us=1173  decode_cpu_us=1173 raw565_parse_us=0
ARC-02 total_us=16504 read_us=305 decode_us=16179 decode_cpu_us=1323 raw565_parse_us=0
ARC-03 total_us=11599 read_us=330 decode_us=11251 decode_cpu_us=1249 raw565_parse_us=0
```

## Diagnosis

The original surprise mostly came from benchmark accounting. Direct decode made
`raw565_parse_us` disappear, but the synthetic benchmark still performed a
full-output checksum walk and charged it to the direct decode path. Removing
zero-fill with uninitialized storage helped allocation cost, but not enough to
create a meaningful end-to-end improvement.

The production-shaped direct `Arc<[u16]>` path removed parse CPU in real worker
traces, but did not materially improve the user-facing first-preview wall time.
The remaining wall spikes appear to be scheduler or worker contention rather
than RGB565 parse/copy CPU work.

## Decision

Drop the entire branch. No code, benchmark option, or diagnostic harness change
from this rerun is worth keeping in production. The added unsafe direct-decode
path and benchmark complexity do not buy enough measurable end-to-end
performance to justify the maintenance cost.
