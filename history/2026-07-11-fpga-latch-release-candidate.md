# FPGA latch release candidate evidence - 2026-07-11

The extracted `0x57`/`0x58` vblank latch candidate has SHA-256
`69e0e312b226c004bfe7fced2cc1145954efa1110cee7a0f58de1528d52627a1`.
GitHub Actions run `29158797784` produced the matched stock/patched Quartus
evidence and returned `quartus_delta_signoff_tsv valid=1`. Fast RTL run
`29159218946` passed simulation, lint, integration/opcode checks, functional
points, and complete reachable custom-module line coverage.

The first performance comparison reused pre-visual-change baseline samples and
correctly failed the 3% Arcade comparison. Repeating the baseline with the same
current binary isolated the RBF variable. The corrected matched comparison
returned `valid=1`: Home baseline/candidate median p99 work was 6799.5/6788.0 us
and Arcade was 5282.0/5374.5 us. All candidate rows had zero latch deadline,
visual, alternation, flip-gap, and unexpected-drop failures.

The exact candidate accepted `0x57` and `0x58`, advanced post/flip counters,
incremented drops under deliberate over-posting, and recovered at normal
cadence. A 1920x1080 USB HDMI capture and contact strips are retained under the
ignored `build/launcher-home-pan-captures/` directory; large raw evidence is not
committed.

Commercial signoff was not reached. The 1280x720 hidden route falls back because
the 1,040,384-byte slots cannot hold a 1,843,200-byte RGB565 frame. The existing
game-return smoke failed to write its return-state marker twice, the two-hour
soak was deferred, and second-unit plus independent review remain outstanding.
The qualification and lifecycle runners were hardened to clear persistent
launcher/fault arming files on failure. No such files remained after testing.
