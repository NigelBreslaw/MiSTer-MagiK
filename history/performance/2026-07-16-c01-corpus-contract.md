# C01 corpus-relative catalog contract

- Parent: `bf6f9fc674a408496dbf5c02409e3005cfd797d2`
- Benchmark class: correctness-only; no performance claim.
- Confirmed cause: the first-scan runner expected 53,457 games and enforced a
  historical 117,766 ms save ceiling, while device acceptance independently
  expected 69,646 durable rows. The current fingerprinted corpus consistently
  produces 53,459 durable rows, 51,102 launcher rows, and 71 systems.

## Before

- Command: `scripts/profile-first-scan.sh C01-CORPUS-BF6F9FC6-BEFORE --skip-build --replace-label --thread-sample`
- Result: failed the fixed game-count contract (`53459 != 53457`) and historical
  save ceiling (`119803 ms > 117766 ms`). RAM readiness was 94,914 ms.
- Command: `scripts/device-catalog-acceptance.sh --label C01-CORPUS-BF6F9FC6-BEFORE --replace-label`
- Result: stopped at the first stale count (`53459 != 69646`).
- Raw evidence: `build/first-scan-profiles/C01-CORPUS-BF6F9FC6-BEFORE-*`
  and `build/catalog-acceptance/C01-CORPUS-BF6F9FC6-BEFORE/`.

## After

- Command: `scripts/profile-first-scan.sh C01-CORPUS-BF6F9FC6-AFTER --skip-build --replace-label --thread-sample`
- Result: fingerprint `962a9ad37c3c49e7` matched the versioned fixture;
  53,459 games/game rows, 51,102 summary/database launcher rows, and 71 systems
  passed. Historical budgets were retained as recorded-only observations.
  RAM readiness was 93,950 ms and save completion was 118,850 ms.
- Command: `scripts/device-catalog-acceptance.sh --label C01-CORPUS-BF6F9FC6-AFTER --replace-label`
- Scoped result: the new corpus contract and games/game_rows,
  summary/navigation-row, and summary/database-system parity checks passed.
- Raw evidence: `build/first-scan-profiles/C01-CORPUS-BF6F9FC6-AFTER-*`
  and `build/catalog-acceptance/C01-CORPUS-BF6F9FC6-AFTER/`.

## Validation and disclosed downstream failure

- `scripts/dev-rust test`: 283 passed.
- `scripts/dev-rust check`: passed.
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`: passed.
- Both modified shell self-tests, the Python contract self-test, `bash -n`, and
  `git diff --check`: passed.
- Full device acceptance remains red on a pre-existing filter-projection parity
  defect that the old early count failure concealed: navigation reports two
  normalized Arcade categories while SQLite reports 66. This change does not
  suppress or weaken that check; it is retained as a blocking downstream
  correctness finding for the catalog-generation work.
