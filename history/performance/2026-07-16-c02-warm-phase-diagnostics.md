# C02 warm catalog phase diagnostics

- Parent: `b82887929b75ca94a33108181a52378a41009fc4`
- Benchmark class: correctness-only; no performance claim.
- Confirmed cause: the warm runner parsed only stamp validation and therefore
  reduced every upstream stall to `missing_stamp_check`. Production had no
  start marker around the synchronous materialized-SQL parity load that blocks
  the hydration-to-validation handoff.

## Before

- Command: `scripts/profile-warm-catalog-start.sh C02-WARM-B8288792-BEFORE --replace-label --iterations 1`
- Result: nonzero with `missing_stamp_check` after the bounded 30-second wait.
- Observable phases: first frame 105 ms, navigation projection/library ready
  3,318 ms, but no explicit projection, parity-start, parity-finish, handoff, or
  fallback diagnosis.
- Raw evidence: `build/warm-catalog/C02-WARM-B8288792-BEFORE-1.log`.

## After

- Command: `scripts/profile-warm-catalog-start.sh C02-WARM-B8288792-AFTER --replace-label --iterations 1`
- Result: expected nonzero diagnostic result `materialized_parity_blocked`.
- Observable phases: first frame 106 ms; navigation projection ready at
  3,220 ms from `navigation_projection`; materialized parity started at
  3,374 ms; no finish or hydration handoff arrived within the same bounded
  30-second window.
- Deployed production binary SHA-256:
  `b5ee50e095d0bdd8d12f684aab4995ef8181befb140302551a837730dfb411d5`.
- Raw evidence: `build/warm-catalog/C02-WARM-B8288792-AFTER-1.log`.

## Validation

- `scripts/profile-warm-catalog-start.sh --self-test`: passed, including
  blocked parity, missing handoff, missing validation start, projection
  fallback, changed catalog, terminal, and budget classifications.
- Targeted hydration-defer Rust test: passed.
- `scripts/dev-rust test`: 283 passed.
- `scripts/dev-rust check`: passed.
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`: passed.
- `cargo fmt --manifest-path magik-gui/Cargo.toml --check` and item-scoped diff
  checks: passed.
- Runtime behavior remains unchanged: the warm path is still blocked by
  materialized parity, and the runner still exits nonzero. Removing that work
  belongs to the later warm-path product commit.
