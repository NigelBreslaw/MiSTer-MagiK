# Agent effectiveness cleanup status

The focused integration is `nigel/agent-effectiveness-tooling`, rebased onto
`9b07180a7b34075a4cfab2c8ed8c4ed935914f77` (`origin/main`) for PR review. It selects only agent guidance, concise
instructions, retired narratives, bounded LSP adoption, local validation planning,
context metrics, the direct-guidance CLI test, and the revised scope plan.

The discarded `nigel/agent-effectiveness` parent branch and its worktree were
removed at the user's request. Their host characterization and extractions are
excluded from this integration. The wanted private LSP revision remains pushed
and checked out independently in the focused worktree. Other worktrees and the
original checkout's unrelated changes were left untouched.

The private LSP repository is pinned to `8b2991e`, which bounds oversized
read-only format and routing failures as well as normal results. A fresh MCP
session using this branch's configuration is required before a running client
uses that revision and its branch/worktree routing.

The original host and device-agent decomposition sequence is superseded by the
replacement roadmap. Launcher/runtime, catalog, and desktop decomposition are
deferred until a concrete maintenance need justifies them. Active 1.0 delivery,
hardware safety procedures, and unresolved qualification evidence remain in
force; the replacement roadmap does not change them. No MagiK 2.0 implementation
or global development-policy migration is included here.

Next work should be bounded to a specific retained system or the future
replacement project after it has scoped instructions. Use the guidance and plan
commands to select local checks, retain CI ownership for expensive assurance,
and keep measurement claims limited to the controlled fixtures recorded in
`agent-effectiveness-measurements.md`.
