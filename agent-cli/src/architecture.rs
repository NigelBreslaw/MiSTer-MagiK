// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic, advisory architecture trend reporting between explicit Git trees.

use crate::error::AgentResult;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA: &str = "mister-magik-architecture-report-v1";

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum ArchitectureCommand {
    /// Report advisory architecture trends between two explicit commits.
    Report {
        #[arg(long)]
        base: String,
        #[arg(long)]
        head: String,
        #[arg(long, value_enum, default_value_t)]
        format: ArchitectureOutput,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ArchitectureOutput {
    #[default]
    Json,
    Markdown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchitectureReport {
    schema: &'static str,
    base: String,
    head: String,
    total_changed_lines: usize,
    advisory_only: bool,
    hotspots: Vec<HotspotReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct HotspotReport {
    owner_id: &'static str,
    path: &'static str,
    intended_destination: &'static str,
    present: bool,
    file_lines: usize,
    largest_function: Option<FunctionSize>,
    mutable_binding_count: usize,
    direct_environment_read_count: usize,
    public_module_count: usize,
    changed_lines: usize,
    change_concentration_basis_points: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct FunctionSize {
    name: String,
    lines: usize,
}

#[derive(Clone, Copy)]
struct Hotspot {
    owner_id: &'static str,
    path: &'static str,
    intended_destination: &'static str,
}

const HOTSPOTS: &[Hotspot] = &[
    Hotspot {
        owner_id: "launcher-runtime",
        path: "apps/mister/src/ui_runner/launcher_loop.rs",
        intended_destination: "P1 Decompose launcher state and frame phases",
    },
    Hotspot {
        owner_id: "host-workflows",
        path: "agent-cli/src/host/mod.rs",
        intended_destination: "P2-A typed host workflow modules",
    },
    Hotspot {
        owner_id: "desktop-app",
        path: "apps/desktop/src/main.rs",
        intended_destination: "P2 next-tier desktop ownership seams",
    },
    Hotspot {
        owner_id: "catalog-persistence",
        path: "crates/catalog/src/sqlite_catalog.rs",
        intended_destination: "P2-B characterization then P3 persistence split",
    },
];

pub fn execute(repository: &Path, command: &ArchitectureCommand) -> AgentResult<()> {
    match command {
        ArchitectureCommand::Report {
            base,
            head,
            format,
            output,
        } => {
            let report = report(repository, base, head)?;
            let rendered = match format {
                ArchitectureOutput::Json => {
                    serde_json::to_string_pretty(&report)
                        .map_err(|error| format!("serialize architecture report: {error}"))?
                        + "\n"
                }
                ArchitectureOutput::Markdown => render_markdown(&report),
            };
            if let Some(path) = output {
                std::fs::write(path, rendered)
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
            } else {
                print!("{rendered}");
            }
        }
    }
    Ok(())
}

pub fn report(repository: &Path, base: &str, head: &str) -> AgentResult<ArchitectureReport> {
    let base = resolve_commit(repository, base)?;
    let head = resolve_commit(repository, head)?;
    let (total_changed_lines, changed_by_path) = changed_lines(repository, &base, &head)?;
    let mut hotspots = Vec::with_capacity(HOTSPOTS.len());
    for hotspot in HOTSPOTS {
        let source = git_text(repository, &["show", &format!("{head}:{}", hotspot.path)])?;
        let present = !source.is_empty();
        let metrics = analyze_source(&source);
        let changed_lines = changed_by_path.get(hotspot.path).copied().unwrap_or(0);
        hotspots.push(HotspotReport {
            owner_id: hotspot.owner_id,
            path: hotspot.path,
            intended_destination: hotspot.intended_destination,
            present,
            file_lines: metrics.file_lines,
            largest_function: metrics.largest_function,
            mutable_binding_count: metrics.mutable_binding_count,
            direct_environment_read_count: metrics.direct_environment_read_count,
            public_module_count: metrics.public_module_count,
            changed_lines,
            change_concentration_basis_points: changed_lines
                .saturating_mul(10_000)
                .checked_div(total_changed_lines)
                .unwrap_or(0),
        });
    }
    Ok(ArchitectureReport {
        schema: SCHEMA,
        base,
        head,
        total_changed_lines,
        advisory_only: true,
        hotspots,
    })
}

struct SourceMetrics {
    file_lines: usize,
    largest_function: Option<FunctionSize>,
    mutable_binding_count: usize,
    direct_environment_read_count: usize,
    public_module_count: usize,
}

fn analyze_source(source: &str) -> SourceMetrics {
    SourceMetrics {
        file_lines: source.lines().count(),
        largest_function: largest_function(source),
        mutable_binding_count: source.match_indices("let mut ").count(),
        direct_environment_read_count: ["env::var(", "env::var_os("]
            .iter()
            .map(|pattern| source.match_indices(pattern).count())
            .sum(),
        public_module_count: source
            .lines()
            .filter(|line| line.trim_start().starts_with("pub mod "))
            .count(),
    }
}

fn largest_function(source: &str) -> Option<FunctionSize> {
    let lines: Vec<_> = source.lines().collect();
    let mut largest = None;
    for (index, line) in lines.iter().enumerate() {
        let Some(name) = function_name(line) else {
            continue;
        };
        let mut depth = 0_i64;
        let mut opened = false;
        let mut end = index;
        for (offset, candidate) in lines[index..].iter().enumerate() {
            let opens = candidate.bytes().filter(|byte| *byte == b'{').count() as i64;
            let closes = candidate.bytes().filter(|byte| *byte == b'}').count() as i64;
            if opens > 0 {
                opened = true;
            }
            depth += opens - closes;
            end = index + offset;
            if opened && depth <= 0 {
                break;
            }
        }
        if opened {
            let candidate = FunctionSize {
                name,
                lines: end - index + 1,
            };
            if largest
                .as_ref()
                .is_none_or(|current: &FunctionSize| candidate.lines > current.lines)
            {
                largest = Some(candidate);
            }
        }
    }
    largest
}

fn function_name(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '#'
        })
        .collect();
    (!name.is_empty()).then_some(name)
}

fn changed_lines(
    repository: &Path,
    base: &str,
    head: &str,
) -> AgentResult<(usize, BTreeMap<String, usize>)> {
    let text = git_text(
        repository,
        &["diff", "--numstat", "--find-renames", base, head, "--"],
    )?;
    let mut total = 0_usize;
    let mut by_path = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(added), Ok(deleted)) = (added.parse::<usize>(), deleted.parse::<usize>()) else {
            continue;
        };
        if excluded_path(path) {
            continue;
        }
        let changed = added.saturating_add(deleted);
        total = total.saturating_add(changed);
        by_path.insert(path.to_owned(), changed);
    }
    Ok((total, by_path))
}

fn excluded_path(path: &str) -> bool {
    path.starts_with("history/")
        || path.starts_with("reference/")
        || path.starts_with("apps/desktop/vendor/")
        || path.starts_with("build/")
        || path.starts_with("dist/")
        || path.contains("/target/")
        || path.contains("/generated/")
        || path.ends_with("/generated.rs")
        || path.contains("/generated_")
        || path.contains("/ui-generated/")
}

fn resolve_commit(repository: &Path, revision: &str) -> AgentResult<String> {
    let resolved = git_text(
        repository,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?;
    let resolved = resolved.trim();
    if resolved.len() != 40 || !resolved.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid architecture report revision: {revision}").into());
    }
    Ok(resolved.to_owned())
}

fn git_text(repository: &Path, args: &[&str]) -> AgentResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        if args.first() == Some(&"show") && output.status.code() == Some(128) {
            return Ok(String::new());
        }
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("git {} returned non-UTF-8 text: {error}", args.join(" ")).into())
}

fn render_markdown(report: &ArchitectureReport) -> String {
    let mut output = format!(
        "# Architecture hotspot summary\n\nBase: `{}`  \nHead: `{}`  \nChanged lines (filtered): {}\n\nThis report is advisory. Lower line counts alone are not success; ownership and dependency direction remain the objective.\n\n| Owner | Path | File lines | Largest function | Mutable bindings | Env reads | Public modules | Changed lines | Concentration | Destination |\n|---|---|---:|---:|---:|---:|---:|---:|---:|---|\n",
        report.base, report.head, report.total_changed_lines
    );
    for hotspot in &report.hotspots {
        let function = hotspot
            .largest_function
            .as_ref()
            .map(|value| format!("{} ({})", value.name, value.lines))
            .unwrap_or_else(|| "—".to_owned());
        output.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} | {} | {} | {:.2}% | {} |\n",
            hotspot.owner_id,
            hotspot.path,
            hotspot.file_lines,
            function,
            hotspot.mutable_binding_count,
            hotspot.direct_environment_read_count,
            hotspot.public_module_count,
            hotspot.changed_lines,
            hotspot.change_concentration_basis_points as f64 / 100.0,
            hotspot.intended_destination
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_metrics_are_deterministic_and_behavior_neutral() {
        let source = r#"pub mod visible;
fn small() {}
fn larger() {
    let mut value = std::env::var("MISTER_VALUE").ok();
    if value.is_none() {
        value = None;
    }
}
"#;
        let metrics = analyze_source(source);
        assert_eq!(metrics.file_lines, 8);
        assert_eq!(metrics.mutable_binding_count, 1);
        assert_eq!(metrics.direct_environment_read_count, 1);
        assert_eq!(metrics.public_module_count, 1);
        assert_eq!(
            metrics.largest_function,
            Some(FunctionSize {
                name: "larger".into(),
                lines: 6
            })
        );
    }

    #[test]
    fn generated_and_historical_paths_do_not_affect_concentration() {
        assert!(excluded_path("history/old.md"));
        assert!(excluded_path("apps/mister/ui-generated/src/lib.rs"));
        assert!(excluded_path("reference/upstream/src/main.rs"));
        assert!(!excluded_path("apps/mister/src/main.rs"));
    }
}
