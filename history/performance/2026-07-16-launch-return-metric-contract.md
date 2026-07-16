# Truthful launch-return metric contract

Benchmark class: correctness-only. No performance claim.

Confirmed cause: `scripts/mister run` can merge a remote command's stderr into
captured stdout. The launch-return wait loop emitted `return_wait_heartbeat` on
stderr, so the old runner parsed that text as `total_return_ms`. Shell integer
checks then errored, but the script continued and falsely printed a passing
two-game summary with empty timing fields.

Before:

- Parent: `92b5b259718944b447fb30d84a79f136f555bcda`.
- Command: `scripts/device-launch-return-smoke.sh`.
- Result: both iterations printed `total_return_ms=return_wait_heartbeat`,
  emitted `integer expected`, left the other timing fields empty, and still
  reported `Device launch return smoke passed`.

After:

- The parser accepts exactly one tab-separated row containing four unsigned
  integer fields and rejects missing, duplicate, or nonnumeric rows.
- `scripts/device-launch-return-smoke.sh --self-test` passes.
- A real device run now rejects rather than false-passes an over-budget return.
  The production confirmation reported 3,570 ms total and 2,404 ms black for
  game 1, correctly exceeding the 3,000/2,000 ms gates.
- This exposes a pre-existing return-startup performance problem for later
  optimization; it does not claim to improve that performance.

Validation:

- Launch-return parser self-test and shell syntax check.
- `scripts/test-host-tools.sh`.
- `scripts/dev-rust test`: 283 passed.
- `scripts/dev-rust check`.
- GUI library clippy and host-tool clippy with `-D warnings`.

Raw evidence:

- `build/launch-return/parser-self-test.log`
- `build/launch-return/C07R-BENCH-FAIL-1.log`
- `build/launch-return/C07R-BENCH-FAIL-2.log`
- `build/launch-return/C07R-PRODUCTION-FAIL.log`

Evidence limitation: the launch-return runner stdout/stderr that exposed the
false pass and later printed the corrected `3570/2404` rejection was observed
live but was not redirected to a raw artifact. The three retained device logs
above are launcher application logs; they corroborate the restored return
context and startup path but do not contain the runner's parser diagnostics.
The deterministic parser self-test is therefore the retained proof of the
merged heartbeat fixture and exact-one-numeric-row contract. No stronger raw
harness-output claim is made.

Review:

- Independent reviewer: `/root/review_launch_return_contract`.
- Reviewed code diff:
  `894d556eb34a2441963bb0bbb34c9558bcd02dfed294866f848c2f088c68d0f0`.
- Reviewed evidence:
  `2bc317d43b1bcf1e2c9dd64bade45780ca3c65a579772602b766ea22904f60a9`.
- Result: approved with no unresolved actionable findings.
