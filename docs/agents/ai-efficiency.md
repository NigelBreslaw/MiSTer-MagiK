# AI development efficiency

Use this guide to keep agent context focused without hiding evidence. Universal
safety remains in `AGENTS.md`; task ownership remains in `task-map.md`.

The project defaults in `.codex/config.toml` retain high reasoning while using
concise summaries and final answers, non-login shells, body-after-prefix
compaction accounting, and a 3,000-token stored tool-output ceiling. These
settings follow OpenAI's current model guidance:

- <https://developers.openai.com/api/docs/guides/latest-model>
- <https://learn.chatgpt.com/docs/config-file/config-reference>

## Inspection loop

1. Start with the narrowest row in `task-map.md` and read only the nearest
   governing `AGENTS.md` plus the listed canonical source.
2. For Rust or Cargo work, use `$magik-rust-lsp` semantic navigation before
   broad shell exploration. Keep broad semantic results to 20 entries without
   snippets unless the skill requires otherwise.
3. Limit initial file reads to 150 lines and broad text searches to 100 matches.
   Narrow by path, symbol, file type, or exact pattern before reading
   more.
4. Reduce output at its source. Prefer bounded `rg` matches, narrow `sed`
   windows, structured `jq`, counts, selected fields, and summaries.
5. If bounded evidence is materially insufficient, make one focused expansion.
   Do not repeat the same broad read with a larger output budget.

## Output budgets

Routine tool calls should return at most 1,200 tokens. The project-level
3,000-token limit is a hard history ceiling, not a routine target. A requested
tool output limit is a backstop; it does not replace source-side selection.

Use `exec_command` for bounded filesystem, Git, build-front-door, and local
process work. Ask commands for the answer needed now rather than an entire log
or file. Store full diagnostic artifacts only in their owning ignored or
temporary location, then return the useful excerpt and artifact reference.

## Programmatic tool reduction

Use programmatic tool calling for filtering, joining, aggregation, validation,
or fan-out whose intermediate results do not require fresh model judgment.
Return only the reduced result. Never forward unconditional broad `r.output`;
filter in the command or JavaScript before calling `text()`.

Direct tool calls remain preferable when one small semantic result is needed or
when the next call depends on model judgment. If a programmatic envelope calls
multiple tools, return labeled per-tool summaries so context attribution stays
auditable.

## Failures and truncation

A compact failure result must include:

- status and failure classification;
- the smallest selected evidence that identifies the problem;
- whether source output or the stored result was truncated;
- the owning artifact location when one exists; and
- the next safe action.

Never hide an actionable failure to meet a token target. Use one focused
follow-up to retrieve the missing evidence.

## Privacy-safe measurement

`scripts/codex-context-report.py` reads local Codex session JSONL and emits only
aggregate counts and sizes. It never prints prompts, responses, command text,
arguments, session identifiers, input paths, or malformed records. Tool token
figures are explicitly estimates based on UTF-8 output bytes.

Use explicit session inputs for a controlled comparison, or inspect recent
local sessions with:

```text
scripts/codex-context-report.py --recent 3
scripts/codex-context-report.py --recent 3 --json
```

For a before/after evaluation, select the same representative investigation
tasks, keep model and reasoning effort fixed, and compare task correctness,
tool-output bytes, estimated tool tokens, compaction count, latency, and
evidence completeness. The target is at least 50% less tool-output data, no
stored result above 3,000 tokens, and no lost actionable evidence. Never commit
session data or generated reports.

## Deliberate exclusions

- Do not set a nominal model context window or fixed compaction threshold in
  project configuration; the active client owns those capabilities.
- Prompt caching is implicit and needs no repository setting.
- Multi-agent concurrency remains user-controlled and is useful only for clean,
  independent workstreams.
- Fast mode remains an explicit user choice because speed and credit cost are a
  separate tradeoff: <https://learn.chatgpt.com/docs/agent-configuration/speed>.
- Plugin and MCP allowlists remain user-level because installed identities are
  not portable across contributors.
