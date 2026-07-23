# Agent CLI performance baseline

This is a tracked snapshot of the legacy local evidence cohort measured during
the agent-cli workflow-efficiency work. The completed cohort was rotated at
revision `8c4fc4dd28516856ab87fc49c21b7b9671b81729`; its raw SQLite database and
captured logs remain ignored under `.agent-cli/archives/`.

- Window: 2026-07-20 20:59:42 through 2026-07-23 18:50:58 local time
- Metrics snapshot revision: `978b1ede33be7e24905321ef2f86c6655d7be893`
- Requests: 2,944
- Commands: 9,425
- Top-level request wall time: 12,970,980 ms
- Recorded command time: 12,657,659 ms
- Request p50/p95: 1 ms / 17,011 ms
- Cache or joined hits: 1,742
- Failed commands: 394
- Requests repeated within 60 seconds: 854

The canonical post-change report is `scripts/agent db report`. Its calculations
exclude child requests from top-level wall time, use `execution_ms` percentiles,
sum command durations separately, count `reused` and `joined` commands as cache
hits, and identify repeated requests by identical redacted `args_json` within
the same cohort and a 60-second window.

Use the typed `scripts/agent db report` interface for subsequent comparisons;
AI workflows must not reconstruct these metrics with ad-hoc SQL.
