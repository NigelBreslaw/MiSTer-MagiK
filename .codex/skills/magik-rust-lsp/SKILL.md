---
name: magik-rust-lsp
description: Use the repository's bounded lspi/rust-analyzer MCP for Rust and Cargo work in MiSTer MagiK. Trigger for editing, reviewing, navigating, diagnosing, or renaming Rust symbols or Cargo projects. Do not trigger for Slint-only, documentation-only, or script-only work.
---

# MiSTer MagiK Rust LSP

Load the MCP lazily for Rust/Cargo work. Include an absolute `file_path` in
workspace searches. Routing uses `rust-desktop` for desktop, `rust-magik-cloud`
for the cloud submodule, and `rust-main` otherwise. Registered Git worktrees
receive isolated analyzer sessions; verify `workspace_root` in returned data.
Branch changes synchronize file content automatically. Let lspi manage lifecycle.

Prefer position-based tools with one-based line/character coordinates. Use
`max_results=20`, `max_total_chars=3000`, and `include_snippet=false` where supported.
Compact results preserve evidence locations, counts, errors, and truncation.
Use `output_format="full"` for legacy detail. Narrow incomplete queries before
one necessary expansion, capped at 20000 characters. Summarize useful findings,
backend/workspace, warnings, and truncation instead of echoing structured JSON.

After coherent edits, request diagnostics for a changed file. Run focused
package/feature checks through `scripts/cargo`; CI owns broad assurance.

For rename, preflight references with `output_format="full"`, `max_results=50`,
and `max_total_chars=10000`. If incomplete, use ordinary patches. Otherwise:

1. Call `rename_symbol_strict` with `dry_run=true` and inspect every affected file.
2. Preserve all preview `before_sha256` values.
3. Apply with `dry_run=false`, the complete `expected_before_sha256` map, and
   `create_backups=false`; reject stale previews.
4. Review the diff and refresh diagnostics.

If MCP is unavailable or fails once, report reduced semantic assurance and
fall back to source navigation and focused Cargo validation. Do not repeatedly
retry or broaden allowed roots to unrelated directories.
