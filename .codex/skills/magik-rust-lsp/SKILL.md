---
name: magik-rust-lsp
description: Use the repository's bounded lspi/rust-analyzer MCP for Rust and Cargo work in MiSTer MagiK. Trigger for editing, reviewing, navigating, diagnosing, or renaming Rust symbols or Cargo projects. Do not trigger for Slint-only, documentation-only, or script-only work.
---

# MiSTer MagiK Rust LSP

Use semantic Rust tools during editing. Leave tests, special feature builds, ARM
validation, and repository assurance to CI; pre-push owns only the bootstrap-free
Python fast gate and quality checks.

## Load And Route

1. Load the `magik-rust-lsp` MCP tools lazily with tool search only after Rust
   or Cargo work is in scope.
2. Route by the file being inspected:
   - `private/magik-cloud/**` uses `rust-magik-cloud`.
   - `apps/desktop/**` uses `rust-desktop`.
   - All other repository Rust uses `rust-main`.
3. Include a representative `file_path` in every workspace-symbol search.
   Longest-root routing selects the backend.
4. Let `lspi` own server lifecycle. File-specific calls start the selected
   backend lazily, and configured idle shutdown releases unused servers.

## Navigate And Diagnose

- Prefer the position-based `*_at` tools with 1-based line and character
  coordinates.
- For broad calls, set `max_results=20`, `max_total_chars=10000`, and
  `include_snippet=false` when supported.
- After a coherent Rust edit batch, request diagnostics for a changed file.
  Rust Analyzer observes file changes and refreshes compiler-backed Clippy
  diagnostics without an agent-managed restart.
- If output is truncated and materially insufficient, make at most one bounded
  follow-up with `max_total_chars` no greater than 20000.
- Report only the useful findings, result count, backend, warnings, and whether
  the response was truncated. Do not reproduce the structured JSON.

## Rename Safely

1. Preflight with `find_references_at`, `max_results=50`,
   `max_total_chars=10000`, and `include_snippet=false`.
2. If references are truncated or exceed the bounded result, use the ordinary
   patch workflow instead of MCP rename.
3. Otherwise call `rename_symbol_strict` with `dry_run=true`.
4. Inspect every affected file and retain the preview's `before_sha256` values.
5. Apply only the unchanged preview using `dry_run=false`, the complete
   `expected_before_sha256` map, and `create_backups=false`.
6. Review the Git diff and refresh diagnostics after applying.

## Failure And Assurance

- Do not invoke repository validation commands. Pre-commit and pre-push own
  bounded fast checks; CI owns formatting, tests, feature matrices, platform
  checks, and full assurance.
- If the MCP is missing or fails once, report reduced edit-time assurance and
  fall back to `rg` for navigation. Do not repeatedly retry or construct Cargo
  validation commands.
