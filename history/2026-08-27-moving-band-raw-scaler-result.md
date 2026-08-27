# Persistent moving-band corruption reaches direct ascal output

## Preservation status

On 2026-08-27 the persistent moving-band/full-raster corruption was physically
visible while MiSTer MagiK remained `LauncherActive`. No reboot, launcher
restart, core or RBF reload, delivery, recovery, or other device mutation was
performed after the operator reported the failure. The device remained in the
failing state while native USB video, three authoritative framebuffer captures,
and two passive FPGA diagnostic bundles were collected.

The ignored evidence root is:

```text
build/agent-diagnostics/2026-08-27-live-corruption-raw-scaler-v3/
```

The compact integrity record is
[`2026-08-27-moving-band-raw-scaler-incident-v1.json`](2026-08-27-moving-band-raw-scaler-incident-v1.json).

## Physical evidence

The fixed native `USB Video` path recorded 105 ordered H.264 frames at
1920x1080 over 15.156667 seconds. The movie shows continuously changing
vertical/line-displaced corruption, including a near-full-raster striped frame
and later bounded colour bands. Its SHA-256 is:

```text
5b6fa5e37cd6fc314a09bbed7fea6f4635aa0f5bf2a20068ef8eb5a91ded3578
```

This is the persistent moving-band class, not a uniform black screen, signal
loss, or the previously observed single-frame transient.

## Static source proof

Three consecutive typed captures from the FPGA-latched scanout slots were
complete, normal 960x540 RGB565 Arcade launcher frames. They are byte-identical
at SHA-256:

```text
7f5765886b3e8e0658f2c01c9f8e150bcf58f3bf6319aa13c645cac8b5930948
```

The second passive latch snapshot reported 821 posts and flips, zero drops,
zero rejects, and no pending transaction. Active base, 960x540 geometry,
1920-byte stride, owner epoch, launcher state, and route ownership were stable.

## Decisive FPGA evidence

Both passive reads were available, coherent, and classified
`raw_scaler_order_changed_requires_static_source_proof` by the installed
schema-10 `raw-scaler-ordered-signature-v3` observer.

- Initial completed raw frames 18027, 18028, and 18030 produced ordered
  signatures `2e7a`, `da6d`, and `4e8e`.
- The static-source window completed raw frames 25494, 25495, and 25497 with
  ordered signatures `750b`, `e31e`, and `ff4a`.
- Every record was valid; ownership and classification were stable across each
  three-sample interval.

Per the frozen schema-10 interpretation contract, changing ordered RGB565 and
line-boundary evidence paired with an independently byte-stable source supports
an origin at or before direct `ascal` output. This rules out corruption created
only by Slint/source rendering, latch presentation, post-ascal routing, or USB
capture. It does not identify the exact fetch, copy, line-buffer, H/V scaler,
or output-alignment transition.

## Exact identity

The immediately preceding typed platform manifest identifies
`platform-v0.31`, MagiK revision
`6806f948ed51f5c06acf8a48f4905fb6f757f429`, Menu revision
`3c3634c0105d78f27aeba66b38966c50dbc42c9b`, latch protocol 5, capabilities
`0x03ff`, and RBF SHA-256
`6c9324ffe52cd6ae10ecac31ef8a37a281dd2882156ecdc4571e5ab5d35a7691`.
The launcher was build 5325 from the same clean MagiK revision. Device boot ID
was 797; Main PID/generation were 581/8480 and launcher PID was 1286.

## Next decision

The next disposable observer must replace schema 10 and ask whether the ordered
accepted-address/returned-data stream at the external scaler Avalon boundary
also changes. It must not stack a wider observer, add internal `ascal` fanout,
or weaken the fixed experimental timing/resource gates. A stable fetch record
does not itself prove an internal scaler cause; it selects the later internal
boundary experiment. A changed fetch record selects address-versus-return
attribution. Recovery and experimental RBF activation remain separate attended
operator decisions.
