# Agent CLI performance baseline

This is the tracked summary of the local evidence cohort archived after the
agent-cli workflow-efficiency work. The raw SQLite database and captured logs
remain ignored under `.agent-cli/archives/`.

- Window: 2026-07-20 20:59:42 through 2026-07-23 18:50:58 local time
- Pre-rotation revision: `978b1ede33be7e24905321ef2f86c6655d7be893`
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

The source database can be independently checked with:

```sql
SELECT datetime(min(started_ms)/1000,'unixepoch','localtime'),
       datetime(max(started_ms)/1000,'unixepoch','localtime'),
       count(*)
FROM requests;

SELECT program, status, count(*), sum(coalesce(duration_ms, 0))
FROM commands
GROUP BY program, status
ORDER BY sum(coalesce(duration_ms, 0)) DESC;
```
