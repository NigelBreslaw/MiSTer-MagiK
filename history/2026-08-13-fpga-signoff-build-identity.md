# FPGA signoff build-identity correction — 2026-08-13

GitHub platform run `31692267364` rejected commit `b018d2b2` for setup-slack
degradation: its internally matched seed-2 baseline measured 0.657 ns and the
final design measured 0.244 ns. Absolute setup and hold timing, zero TNS,
unconstrained-output equality, CDC evidence, and resource gates otherwise
passed. The candidate was not published.

The earlier local result was not a valid comparison with that GitHub run.
Local stock and pre-observer cache entries used build date `260812`, while the
final entry used `260813`; the Menu build date is synthesized into the design,
but the cache marker did not bind it. Local preparation also disabled parallel
synthesis and used nine available Quartus processors, while GitHub left the
parallel-synthesis defaults enabled and exposed four processors. The seed and
source hashes therefore did not describe all implementation inputs.

The correction makes one tracked preparation script authoritative for local
and GitHub Menu checkouts, fixes `NUM_PARALLEL_PROCESSORS` to four, disables both
parallel-synthesis settings, and binds the synthesis revision date,
preparation-script blob, processor count, and complete synthesis component ID
into the local per-variant cache marker. The delta checker requires the same
three Quartus assignments in stock, pre-observer, and final reports. Cache
format v4 intentionally prevents migration of the earlier unmatched results.

No observer RTL, timing threshold, CDC exception, seed, or warning waiver was
changed. Qualification requires one new fully matched local signoff followed
by one GitHub run from the same committed policy.
