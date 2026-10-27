//! The fmt and clippy phases.
//!
//! `cargo fmt --check` has no useful machine-readable progress; we run it and
//! treat a non-zero exit as "formatting issues". `cargo clippy` is run with
//! `--message-format=json` so we can count errors and warnings precisely from
//! the compiler-message stream rather than scraping human text.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::config::Scope;
use crate::phase::{Status, Step, StepResult};
use crate::runner::run_with_capture;

/// A cargo JSON message line. We only care about `compiler-message` entries.
#[derive(Debug, Deserialize)]
struct CargoJsonLine {
    reason: String,
    message: Option<CompilerMessage>,
}

#[derive(Debug, Deserialize)]
struct CompilerMessage {
    #[serde(default)]
    level: String,
    #[serde(default)]
    rendered: Option<String>,
}

/// Build the `cargo fmt --check` argument vector.
fn fmt_args() -> Vec<String> {
    vec!["fmt".to_string(), "--check".to_string()]
}

/// Build the clippy argument vector for a scope, in JSON message format.
///
/// Clippy shares the SAME package-selection chokepoint as the test phase
/// (`nextest::scope_args`), so the default-members behaviour is identical across
/// fmt/clippy/nextest: `Scope::DefaultMembers` emits no `--workspace`/`-p` and
/// Cargo falls back to the workspace default-members set. Clippy takes no orphan
/// excludes (the gate never passed them), so an empty orphan list is used.
fn clippy_args(scope: &Scope) -> Vec<String> {
    let mut args = vec!["clippy".to_string()];
    args.extend(crate::phases::nextest::scope_args(scope, &[]));
    // JSON diagnostics on stdout.
    args.push("--message-format=json".to_string());
    args
}

/// Run the fmt phase.
pub fn run_fmt(
    cargo: &str,
    cwd: &Path,
    stdout_log: &Path,
    stderr_log: &Path,
    timeout: Duration,
    mut tick: impl FnMut(),
) -> Result<StepResult> {
    let out = run_with_capture(
        cargo,
        &fmt_args(),
        cwd,
        stdout_log,
        stderr_log,
        &[],
        timeout,
        |_line| tick(),
    )?;

    let status = if out.timed_out {
        Status::TimedOut
    } else if out.success() {
        Status::Ok
    } else {
        Status::Issues
    };

    let mut review_lines = Vec::new();
    let (errors, _warnings) = if status == Status::Issues {
        review_lines.push("fmt: formatting differences found (run `cargo fmt`)".to_string());
        (1u64, 0u64)
    } else {
        (0, 0)
    };

    Ok(StepResult {
        step: Step::Fmt,
        status,
        duration: out.elapsed,
        warnings: 0,
        errors,
        items_total: None,
        items_passed: None,
        items_failed: None,
        review_lines,
    })
}

/// Run the clippy phase, counting diagnostics from the JSON message stream.
///
/// Note: clippy's JSON messages go to **stdout**, so we parse the stdout log
/// after the run; the live `tick` just advances the spinner.
pub fn run_clippy(
    cargo: &str,
    cwd: &Path,
    scope: &Scope,
    stdout_log: &Path,
    stderr_log: &Path,
    timeout: Duration,
    mut tick: impl FnMut(),
) -> Result<StepResult> {
    let out = run_with_capture(
        cargo,
        &clippy_args(scope),
        cwd,
        stdout_log,
        stderr_log,
        &[],
        timeout,
        |_line| tick(),
    )?;

    let raw = std::fs::read_to_string(stdout_log).unwrap_or_default();
    let counts = count_diagnostics(&raw);

    let status = if out.timed_out {
        Status::TimedOut
    } else if !out.success() || counts.errors > 0 {
        Status::Issues
    } else {
        Status::Ok
    };

    Ok(StepResult {
        step: Step::Clippy,
        status,
        duration: out.elapsed,
        warnings: counts.warnings,
        errors: counts.errors,
        items_total: None,
        items_passed: None,
        items_failed: None,
        review_lines: counts.review_lines,
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DiagnosticCounts {
    warnings: u64,
    errors: u64,
    review_lines: Vec<String>,
}

/// Parse cargo/clippy JSON output, counting `compiler-message` entries by level
/// and collecting the rendered text of errors and warnings for the review log.
fn count_diagnostics(raw: &str) -> DiagnosticCounts {
    let mut counts = DiagnosticCounts::default();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let parsed: CargoJsonLine = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if parsed.reason != "compiler-message" {
            continue;
        }
        let Some(msg) = parsed.message else { continue };
        match msg.level.as_str() {
            "error" | "error: internal compiler error" => {
                counts.errors += 1;
                if let Some(r) = msg.rendered {
                    counts.review_lines.push(first_line(&r).to_string());
                }
            }
            "warning" => {
                counts.warnings += 1;
                if let Some(r) = msg.rendered {
                    counts.review_lines.push(first_line(&r).to_string());
                }
            }
            _ => {}
        }
    }
    counts
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clippy_args_default_members_emit_no_workspace() {
        // Validates: Requirement 16.1 -- the default scope emits NO --workspace
        // (and no -p) for clippy, so Cargo falls back to default-members.
        let args = clippy_args(&Scope::DefaultMembers);
        assert!(!args.contains(&"--workspace".to_string()));
        assert!(!args.contains(&"-p".to_string()));
        assert!(args.contains(&"--message-format=json".to_string()));
    }

    #[test]
    fn clippy_args_use_workspace_when_requested() {
        // Validates: Requirement 16.2 -- --workspace selects the full member set
        // for clippy.
        let args = clippy_args(&Scope::Workspace);
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"--message-format=json".to_string()));
    }

    #[test]
    fn clippy_args_target_single_crate() {
        let args = clippy_args(&Scope::Crate {
            name: "ff-keys".to_string(),
        });
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"ff-keys".to_string()));
        assert!(!args.contains(&"--workspace".to_string()));
    }

    #[test]
    fn counts_warnings_and_errors_from_json() {
        let json = r#"
{"reason":"compiler-message","message":{"level":"warning","rendered":"warning: unused variable `x`\n --> src/lib.rs:1:1"}}
{"reason":"compiler-message","message":{"level":"error","rendered":"error[E0425]: cannot find value `y`\n --> src/lib.rs:2:2"}}
{"reason":"compiler-artifact","package_id":"foo"}
{"reason":"compiler-message","message":{"level":"note","rendered":"note: ..."}}
"#;
        let counts = count_diagnostics(json);
        assert_eq!(counts.warnings, 1);
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.review_lines.len(), 2);
        assert!(counts.review_lines[0].starts_with("warning: unused variable"));
        assert!(counts.review_lines[1].starts_with("error[E0425]"));
    }

    #[test]
    fn ignores_non_json_and_non_message_lines() {
        let text =
            "Compiling foo v0.1.0\nFinished dev\n{\"reason\":\"build-finished\",\"success\":true}";
        let counts = count_diagnostics(text);
        assert_eq!(counts, DiagnosticCounts::default());
    }

    #[test]
    fn first_line_truncates_multiline_rendered() {
        assert_eq!(first_line("a\nb\nc"), "a");
        assert_eq!(first_line("only"), "only");
    }
}
