//! Long-format per-step run history and per-step ETA.
//!
//! The CSV has one row **per step per run**, so each phase (fmt, clippy, build,
//! test) can be trended and predicted independently:
//!
//! ```text
//! run_id,timestamp,mode,scope,step,duration_secs,status,warnings,errors,items_total,items_passed,items_failed
//! ```
//!
//! The ETA for a step is the median of that step's recent durations for the same
//! mode and scope, so a quick `--fast --crate x` run is not predicted from a
//! full `--workspace` history.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::phase::{Step, StepResult};

pub const HEADER: &str =
    "run_id,timestamp,mode,scope,step,duration_secs,status,warnings,errors,items_total,items_passed,items_failed";

/// One parsed history row (only the fields the ETA needs are typed).
#[derive(Debug, Clone)]
struct Row {
    mode: String,
    scope: String,
    step: String,
    duration_secs: f64,
}

/// Append one row per step result to the history CSV, creating the file with a
/// header if needed.
pub fn append_rows(
    path: &Path,
    run_id: &str,
    timestamp: &str,
    mode: &str,
    scope: &str,
    results: &[StepResult],
) -> Result<()> {
    let needs_header = !path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open history CSV {path:?}"))?;
    if needs_header {
        writeln!(file, "{HEADER}")?;
    }
    for r in results {
        let line = format_row(run_id, timestamp, mode, scope, r);
        writeln!(file, "{line}")?;
    }
    Ok(())
}

fn format_row(run_id: &str, timestamp: &str, mode: &str, scope: &str, r: &StepResult) -> String {
    let secs = format!("{:.1}", r.duration.as_secs_f64());
    let opt = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_default();
    format!(
        "{run_id},{timestamp},{mode},{scope},{step},{secs},{status},{warnings},{errors},{total},{passed},{failed}",
        step = r.step.id(),
        status = r.status.as_str(),
        warnings = r.warnings,
        errors = r.errors,
        total = opt(r.items_total),
        passed = opt(r.items_passed),
        failed = opt(r.items_failed),
    )
}

/// Median duration for a given step under the same mode and scope, over the most
/// recent `window` matching rows. Returns None when there is no history.
pub fn eta_for_step(
    path: &Path,
    step: Step,
    mode: &str,
    scope: &str,
    window: usize,
) -> Option<Duration> {
    let rows = read_rows(path).ok()?;
    let mut matching: Vec<f64> = rows
        .iter()
        .filter(|r| r.mode == mode && r.scope == scope && r.step == step.id())
        .map(|r| r.duration_secs)
        .collect();
    if matching.is_empty() {
        return None;
    }
    // Keep the most recent `window` entries (rows are in file order == chrono).
    if matching.len() > window {
        matching = matching.split_off(matching.len() - window);
    }
    matching.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = matching.len() / 2;
    let median = if matching.len() % 2 == 1 {
        matching[mid]
    } else {
        (matching[mid - 1] + matching[mid]) / 2.0
    };
    Some(Duration::from_secs_f64(median))
}

/// The history `step` label for a whole-run wall-clock duration row. This is NOT
/// a `Step` phase: it records the END-TO-END run time (including the untimed
/// `nextest list` pre-pass, workspace discovery, and inter-phase overhead) so
/// the overall ETA predicts the same span the completion footer measures.
pub const OVERALL_STEP: &str = "overall";

/// Append a single whole-run row recording the total wall-clock `duration` under
/// `step = "overall"`. Written once per run alongside the per-phase rows, so the
/// headline ETA can be learned from past TOTAL times (which include the list
/// pre-pass and overhead that the sum of per-phase medians misses). Reuses the
/// existing CSV schema -- "overall" is just another step label, no new columns.
pub fn append_overall_row(
    path: &Path,
    run_id: &str,
    timestamp: &str,
    mode: &str,
    scope: &str,
    duration: Duration,
) -> Result<()> {
    let needs_header = !path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open history CSV {path:?}"))?;
    if needs_header {
        writeln!(file, "{HEADER}")?;
    }
    let secs = format!("{:.1}", duration.as_secs_f64());
    writeln!(
        file,
        "{run_id},{timestamp},{mode},{scope},{OVERALL_STEP},{secs},ok,0,0,,,"
    )?;
    Ok(())
}

/// Headline run ETA: the median of past WHOLE-RUN ("overall") durations for the
/// same mode and scope. This is consistent with the completion footer's "actual"
/// (both are end-to-end wall clock), so the variance reflects real drift rather
/// than the gap between a sum-of-phases estimate and wall-clock time. Returns
/// None when no "overall" history exists yet (e.g. the first run after
/// upgrading); callers may fall back to `overall_eta` for a rough first estimate.
pub fn overall_eta_total(path: &Path, mode: &str, scope: &str, window: usize) -> Option<Duration> {
    let rows = read_rows(path).ok()?;
    let mut matching: Vec<f64> = rows
        .iter()
        .filter(|r| r.mode == mode && r.scope == scope && r.step == OVERALL_STEP)
        .map(|r| r.duration_secs)
        .collect();
    if matching.is_empty() {
        return None;
    }
    if matching.len() > window {
        matching = matching.split_off(matching.len() - window);
    }
    matching.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = matching.len() / 2;
    let median = if matching.len() % 2 == 1 {
        matching[mid]
    } else {
        (matching[mid - 1] + matching[mid]) / 2.0
    };
    Some(Duration::from_secs_f64(median))
}

/// Rough fallback estimate for a run with no "overall" history yet: the sum of
/// the per-step median ETAs for every phase. Returns None if ANY phase lacks
/// history. This UNDER-estimates a real run (it omits the list pre-pass and
/// inter-phase overhead), so it is used only until a true "overall" row exists;
/// `overall_eta_total` supersedes it from the second run onward. When
/// `include_app_build` is true the optional app-build phase is part of the sum.
pub fn overall_eta(
    path: &Path,
    mode: &str,
    scope: &str,
    window: usize,
    include_app_build: bool,
) -> Option<Duration> {
    let mut steps = vec![Step::Fmt, Step::Clippy, Step::Build, Step::Test];
    if include_app_build {
        steps.push(Step::AppBuild);
    }
    let mut total = Duration::ZERO;
    for step in steps {
        let step_eta = eta_for_step(path, step, mode, scope, window)?;
        total += step_eta;
    }
    Some(total)
}

fn read_rows(path: &Path) -> Result<Vec<Row>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if i == 0 || line.trim().is_empty() {
            continue; // header or blank
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 {
            continue;
        }
        let duration_secs = f[5].parse::<f64>().unwrap_or(0.0);
        rows.push(Row {
            mode: f[2].to_string(),
            scope: f[3].to_string(),
            step: f[4].to_string(),
            duration_secs,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::{Status, Step};

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("cargo-gate-hist-{name}-{}.csv", std::process::id()));
        let _ = std::fs::remove_file(&d);
        d
    }

    fn result(step: Step, secs: f64) -> StepResult {
        StepResult::new(step, Status::Ok, Duration::from_secs_f64(secs))
    }

    #[test]
    fn writes_header_once_and_one_row_per_step() {
        let path = tmp("header");
        let r = vec![result(Step::Fmt, 1.0), result(Step::Test, 10.0)];
        append_rows(
            &path,
            "run1",
            "2026-01-01 00:00:00",
            "FULL",
            "workspace",
            &r,
        )
        .unwrap();
        append_rows(
            &path,
            "run2",
            "2026-01-01 00:01:00",
            "FULL",
            "workspace",
            &r,
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines[0], HEADER);
        // 1 header + 2 rows * 2 runs = 5 lines.
        assert_eq!(lines.len(), 5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn eta_is_median_of_matching_step_mode_scope() {
        let path = tmp("eta");
        for (run, secs) in [("r1", 10.0), ("r2", 20.0), ("r3", 30.0)] {
            append_rows(
                &path,
                run,
                "2026-01-01 00:00:00",
                "FULL",
                "workspace",
                &[result(Step::Test, secs)],
            )
            .unwrap();
        }
        let eta = eta_for_step(&path, Step::Test, "FULL", "workspace", 10).unwrap();
        assert!(
            (eta.as_secs_f64() - 20.0).abs() < 0.01,
            "median of 10,20,30 is 20"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn eta_none_when_no_matching_history() {
        let path = tmp("none");
        append_rows(
            &path,
            "r1",
            "2026-01-01 00:00:00",
            "FULL",
            "workspace",
            &[result(Step::Test, 10.0)],
        )
        .unwrap();
        // Different scope -> no match.
        assert!(eta_for_step(&path, Step::Test, "FULL", "crate:x", 10).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overall_eta_sums_four_phase_medians() {
        let path = tmp("overall-sum");
        let steps = [
            (Step::Fmt, 1.0),
            (Step::Clippy, 2.0),
            (Step::Build, 3.0),
            (Step::Test, 4.0),
        ];
        for (step, secs) in steps {
            append_rows(&path, "r1", "t", "FULL", "workspace", &[result(step, secs)]).unwrap();
        }
        let eta = overall_eta(&path, "FULL", "workspace", 10, false).unwrap();
        assert!(
            (eta.as_secs_f64() - 10.0).abs() < 0.01,
            "1+2+3+4 = 10 seconds"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overall_eta_none_when_any_phase_missing() {
        let path = tmp("overall-missing");
        // Only three of the four phases have history; test is absent.
        for (step, secs) in [(Step::Fmt, 1.0), (Step::Clippy, 2.0), (Step::Build, 3.0)] {
            append_rows(&path, "r1", "t", "FULL", "workspace", &[result(step, secs)]).unwrap();
        }
        assert!(overall_eta(&path, "FULL", "workspace", 10, false).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overall_eta_includes_app_build_when_requested() {
        // Validates: Requirement 15.5 -- the overall ETA accounts for the
        // optional app-build phase when --build is on.
        let path = tmp("overall-appbuild");
        let steps = [
            (Step::Fmt, 1.0),
            (Step::Clippy, 2.0),
            (Step::Build, 3.0),
            (Step::Test, 4.0),
            (Step::AppBuild, 5.0),
        ];
        for (step, secs) in steps {
            append_rows(&path, "r1", "t", "FULL", "workspace", &[result(step, secs)]).unwrap();
        }
        let eta = overall_eta(&path, "FULL", "workspace", 10, true).unwrap();
        assert!(
            (eta.as_secs_f64() - 15.0).abs() < 0.01,
            "1+2+3+4+5 = 15 seconds with app-build included"
        );
        // Without include_app_build the sum ignores the app-build row.
        let four = overall_eta(&path, "FULL", "workspace", 10, false).unwrap();
        assert!((four.as_secs_f64() - 10.0).abs() < 0.01);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overall_eta_none_when_app_build_requested_but_missing() {
        // Validates: Requirement 14.3 -- unknown ETA when a selected phase lacks
        // history; here --build is on but no app-build row exists yet.
        let path = tmp("overall-appbuild-missing");
        let steps = [
            (Step::Fmt, 1.0),
            (Step::Clippy, 2.0),
            (Step::Build, 3.0),
            (Step::Test, 4.0),
        ];
        for (step, secs) in steps {
            append_rows(&path, "r1", "t", "FULL", "workspace", &[result(step, secs)]).unwrap();
        }
        assert!(overall_eta(&path, "FULL", "workspace", 10, true).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overall_eta_total_is_median_of_whole_run_rows() {
        // Validates: the headline ETA is learned from past WHOLE-RUN durations,
        // not the sum of per-phase medians, so it matches the "actual" wall clock.
        let path = tmp("overall-total");
        for (run, secs) in [("r1", 100.0), ("r2", 300.0), ("r3", 500.0)] {
            append_overall_row(
                &path,
                run,
                "2026-01-01 00:00:00",
                "FULL",
                "workspace",
                Duration::from_secs_f64(secs),
            )
            .unwrap();
        }
        let eta = overall_eta_total(&path, "FULL", "workspace", 10).unwrap();
        assert!(
            (eta.as_secs_f64() - 300.0).abs() < 0.01,
            "median of 100,300,500 is 300"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overall_eta_total_none_without_overall_rows() {
        // Only per-phase rows exist; the whole-run ETA must be None so the caller
        // can fall back to the rough sum-of-phases estimate on the first run.
        let path = tmp("overall-total-none");
        append_rows(
            &path,
            "r1",
            "t",
            "FULL",
            "workspace",
            &[result(Step::Fmt, 1.0)],
        )
        .unwrap();
        assert!(overall_eta_total(&path, "FULL", "workspace", 10).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn eta_respects_recent_window() {
        let path = tmp("window");
        // Older huge values, then recent small ones; window=2 should ignore old.
        for secs in [1000.0, 1000.0, 4.0, 6.0] {
            append_rows(
                &path,
                "r",
                "t",
                "FULL",
                "workspace",
                &[result(Step::Build, secs)],
            )
            .unwrap();
        }
        let eta = eta_for_step(&path, Step::Build, "FULL", "workspace", 2).unwrap();
        assert!(
            (eta.as_secs_f64() - 5.0).abs() < 0.01,
            "median of last two (4,6) is 5"
        );
        let _ = std::fs::remove_file(&path);
    }
}
