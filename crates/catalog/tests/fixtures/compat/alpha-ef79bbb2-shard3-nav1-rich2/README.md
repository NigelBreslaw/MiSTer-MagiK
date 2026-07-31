# Alpha catalog predecessor fixture

This immutable corpus describes the persisted format produced by the public
`alpha` tag at revision `ef79bbb2a46fe35a7182db2161457129ae9fa7d2` on
2026-07-28. It is deliberately stored as the decoded navigation envelope and
its associated authority metadata so compatibility tests can encode it with
the historical schema without depending on old executable code.

The predecessor is canonical schema/build 66/15, shard schema 3, navigation
schema 1, manifest schema 1, binding schema 1, state/scanner schema 1, builder
protocol 1, and projection contract `rich-game-v2`. Navigation v1 has no
`category` field. Tests must treat this directory as read-only and must verify
the hashes recorded in `provenance.json` before using it.
