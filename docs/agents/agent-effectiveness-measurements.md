# Agent effectiveness measurements

Implementation baseline: `367823f48fe55e75b952384d1b43357fe38a8533`.
Focused integration branch: `nigel/agent-effectiveness-tooling`, created in a
separate worktree from that baseline. The preserved
`nigel/agent-effectiveness` branch contains earlier host extractions and is not
part of this comparison.

## Instruction ancestry fixtures

These controlled scenarios count whitespace-delimited words and UTF-8 bytes in
tracked repository `AGENTS.md` ancestor chains. They exclude global
instructions, skills, model tokens, and the actual context injected into a
session. The before revision is the implementation baseline; the after revision
is the instruction-cleanup revision `2c0e277db039842205cf081e450dab920c3b4d8f`.

| Scenario | Before words | After words | Change | Before bytes | After bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Documentation (`docs/catalog.md`) | 749 | 432 | -42.3% | 5,842 | 3,512 |
| Portable Rust (`crates/catalog/src/sqlite_catalog.rs`) | 800 | 460 | -42.5% | 6,240 | 3,753 |
| UI (`apps/mister/src/ui_runner/launcher_loop.rs`) | 1,093 | 607 | -44.5% | 8,420 | 4,914 |
| Host workflow (`agent-cli/src/host/mod.rs`) | 749 | 432 | -42.3% | 5,842 | 3,512 |
| Recovery planning (`/tmp/mister-magik/fs-fault.json`) | 749 | 432 | -42.3% | 5,842 | 3,512 |

The tracked repository now has nine `AGENTS.md` files with 992 words in total;
the root file has 432 words. These are editorial measurements, not claims about
prompt size, productivity, or runtime performance. Reproduce with the
privacy-safe receipt `/private/tmp/magik-instruction-scenarios.json` while it is
available, or by counting the same ancestor chains at the recorded revisions.

## LSP bounded-output fixture

A six-symbol controlled fixture serialized the following results. Values are
serialized UTF-8 bytes, not a repository-wide context measurement.

| Format and requested budget | Symbols returned | Payload bytes | MCP result bytes | Truncated |
| --- | ---: | ---: | ---: | --- |
| Full, 20,000 | 6 | 4,474 | 4,575 | No |
| Full, 3,000 | 3 | 2,565 | 2,697 | Yes |
| Compact, 3,000 | 6 | 2,165 | 2,266 | No |
| Invalid format with 10,000-character synthetic input, 3,000 | — | 283 | — | Explicit error |

The compact fixture payload is 51.6% smaller than the equivalent full payload
at the 20,000-character budget. The final row verifies that a request-format
failure now receives the same cap. Registered-worktree routing has a matching
dispatch regression and smoke test. These results do not measure productivity,
loaded context, or physical performance.

The current connected MCP process was started from the original checkout and
has not been restarted. Start a fresh MCP session using this branch's
configuration to activate the compact defaults and branch/worktree routing.

## Available and unavailable session evidence

The existing privacy-safe context reporter can read incomplete records without
publishing prompts, tool arguments, secrets, or raw session contents. Its
available baseline record has 27 context samples, 19,143 initial input tokens,
198,660 final input tokens, no tool-size samples, and 27 records classified as
unknown. There is no matched focused-branch session comparison, so session
savings and productivity effects are unavailable.

```sh
python3 scripts/codex-context-report.py --recent 1 --json
```

## Architecture metrics

The earlier architecture report measured complete responsibility families so
moving code could not appear to remove complexity. Its original baseline values
remain useful for future maintenance, but this focused branch intentionally does
not integrate host, device-agent, launcher/runtime, desktop, or catalog
extractions. No final decomposition comparison is reported here.

Hardware qualification debt remains unresolved and is outside these tooling
measurements. Run only the focused checks named by the affected workflow; CI
continues to own Linux, ARM, feature-matrix, and expensive assurance.
