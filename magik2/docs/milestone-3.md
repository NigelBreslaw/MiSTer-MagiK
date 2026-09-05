# Milestone 3: the everyday development path

## Scope and commit plan

1. Make `scripts/magik2` default to real MagiK for build/deploy/check/watch;
   Mini remains explicit with `--app mini-magik`. Default checks run one smoke
   journey, not benchmark windows. Print the selected executable/data paths and
   retained failure evidence. Update root developer guidance.
2. Exercise Home → Settings → Home using Slint keyboard events and the existing
   typed application action queue. Retain response timings and a screenshot in
   the same pytest result, without a second benchmark runner or performance gate.
3. Correct Linux process-name truncation in the legacy-agent status check, then
   verify the ordinary workflow while the old agent is absent. No uninstall or
   persistent startup changes. Add only focused regression checks.
4. Record acceptance and a source-backed legacy consumer disposition inventory.

## Definition of done

The ordinary CLI targets the Dev app/data explicitly; smoke exercises real
navigation; a failure points to retained logs; bounded device acceptance records
legacy-agent absence before and after. Existing idle profiling and viewer paths
are exercised once only as needed to establish independence. No transfer or
Mini benchmarks, performance tuning loops, platform changes or broad local
assurance. Necessary app restarts are authorized.

## Explicit exclusions

Do not port matrices, endurance loops, workflow databases, qualification ladders,
rollback gates, version matching, historical experiments or separate benchmark
runners. FPGA production, downloader publication and data releases stay outside
this development tool. The consumer inventory is a deletion decision list, not
an implementation backlog. The legacy CLI and installed agent are not uninstalled
in this milestone.

## Acceptance

Pending implementation and bounded device verification.
