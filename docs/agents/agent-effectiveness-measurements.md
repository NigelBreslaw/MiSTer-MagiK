# Agent effectiveness measurements

Implementation baseline: `367823f48fe55e75b952384d1b43357fe38a8533`.
Branch: `nigel/agent-effectiveness`, in a separate linked worktree.

| Owner | Facade lines | Subsystem lines | Rust files |
| --- | ---: | ---: | ---: |
| Launcher runtime | 18,995 | 59,282 | 51 |
| Host workflows | 44,756 | 59,192 | 15 |
| Desktop | 6,766 | 13,518 | 13 |
| Catalog persistence | 7,519 | 7,519 | 1 |
| Launcher state | 10,312 | 10,312 | 1 |
| Device agent | 9,328 | 10,749 | 6 |

Metrics include complete responsibility families, including pre-existing helpers.
The report keeps facade metrics for compatibility and adds aggregate `subsystem`
metrics. Function lengths are lexical estimates, not a Rust complexity analysis.
Moving implementation into another counted file does not remove its cost.

Reproduce the baseline and final comparison from the implementation checkout:

```sh
scripts/magik-ci architecture report --base 367823f48fe55e75b952384d1b43357fe38a8533 --head 367823f48fe55e75b952384d1b43357fe38a8533 --format markdown
scripts/magik-ci architecture report --base 367823f48fe55e75b952384d1b43357fe38a8533 --head HEAD --format markdown
python3 scripts/codex-context-report.py --recent 1 --json
```

For session comparisons, pass the same explicitly selected session files to the
existing context reporter. Its output includes only aggregate counts and sizes;
do not publish session files, prompts, arguments, or raw tool contents. Incomplete
records remain readable. The available baseline session has 27 context samples,
19,143 initial input tokens, 198,660 final input tokens, and no tool-size samples;
27 records are classified as unknown. This is not a matched productivity trial.

Instruction baseline: ten AGENTS.md files, 1,838 words; root 749 words. After the
documentation cleanup: nine files, 992 words; root 432 words. Count tracked AGENTS.md
files at each revision, excluding independent submodule contents.

Representative checks for final comparison: guidance and planning for
`docs/catalog.md`, `crates/catalog/src/sqlite_catalog.rs`,
`apps/mister/src/ui_runner/launcher_loop.rs`, `agent-cli/src/host/mod.rs`, and
`/tmp/mister-magik/fs-fault.json` (guidance only for this device-state path).
These are read-only planning scenarios; they do not contact a device. LSP budget
and worktree fixtures measure serialized output and identity independently.
No physical-performance or productivity improvement follows from file splitting.
