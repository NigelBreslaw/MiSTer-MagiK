# Catalog finality pipeline baseline — 2026-08-21

This is the pre-pipeline whole-card baseline after the Neon68K/X68000 discovery
regression was fixed. It uses the exact-identity inspection schema introduced for
the finality pipeline and the normal configured library sources on the MiSTer SD
card.

## Corpus inventory

`scripts/agent benchmark catalog-corpus-inventory` passed against revision
`5408cd0046ecc7b9061da3a803b891997fbd6063` with 161 planned scan targets.

- Neon68K prepared collection: 273 MGL candidates in 273 files under
  `/media/fat/_Computer/_X68000 Games`.
- Runtime X68000 target: 4 candidates across 284 files and 275 directories under
  `/media/fat/games/X68000`.
- Evidence: `build/agent-benchmarks/catalog-corpus-inventory/1787297770/summary.json`.

## Whole-card rebuild

`scripts/agent benchmark catalog-full-build-rebuild` passed against revision
`9aec573d2ccc743179f3752301840fb14c82d54c`. All three legs published 40,059
games across 69 systems. X68000 contributed 273 source games and published 269
visible families after four deterministic variant collapses.

| Leg | Complete | First visible | Peak HWM |
|---|---:|---:|---:|
| First observed clean | 169.478 s | 15.598 s | 134,776 KiB |
| Warm clean | 204.705 s | 22.242 s | 142,556 KiB |
| Forced rebuild | 79.291 s | 3.003 s | 63,856 KiB |

The maximum observed high-water mark was 142,556 KiB, below the 144,328 KiB
acceptance ceiling.

## Exact identity gates

The benchmark report schema is `mister-magik-catalog-full-build-rebuild-v3`.
It passed `exact_identities_identical`, `artifact_sets_valid`, and
`phase_evidence_complete`.

- Canonical rows: `db6656f7411281914b62e6345f655a963635d8d98bcc8823045e314edf4b6133`
- Ordering: `5f2ddc9bdb6b6d28dd49741b9e756985a1aa25c96a2c46285e2c00cd36a5e9d0`
- Launch contracts: `316b0858e2bb59b39c0e5f98e902f539889d364aad9be17ed06fc7064651cc47`
- Persisted search: `9932dd85e96e2de00279f7586a724bf35713b0f5cf3ce3de9f067f6a2f333147`
- Artifact set: `95ff70bdb6df2e6708cf09756a1c45b36c5ad563a7d3ca32c51c282a63f293ca`

Evidence is in
`build/agent-benchmarks/catalog-full-build-rebuild/1787299534/summary.json` and
the adjacent per-leg inspection, launcher, telemetry, and Markdown report files.

## Contributor-closure qualification

Revision `6419d2e775e3f543d1d56a197dc192727f3b7ac0` added conservative
contributor-set closure telemetry without starting early shard work. A second
whole-card run passed with 74 discovered systems closed, zero unknown
contributors remaining, and `sound=1` on the complete scan in every leg. Exact
identities and artifact sets remained equal.

The closure-qualified run completed in 197.555 s fresh, 144.541 s warm clean,
and 54.039 s forced rebuild. Evidence is in
`build/agent-benchmarks/catalog-full-build-rebuild/1787300445/summary.json`.
