//! Gate-run summary sourced (read-only) from the long-format `gate.history.csv`.
//!
//! This reader is intentionally separate from `history::read_rows` (which only
//! keeps the fields the ETA needs): the summary needs the per-step status and
//! test-count columns. It never writes the CSV and does not change its columns.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Duration;

use crate::phase::{Status, Verdict};
use crate::report::format_hms;

// === Types ==========================================================

/// Test counts for the summarised run, when the columns were populated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCounts {
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
}

/// One summarised step of the latest run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSummary {
    pub step: String,
    pub duration: Duration,
    pub status: String,
}

impl StepSummary {
    /// The step duration formatted as H:MM:SS.
    pub fn duration_hms(&self) -> String {
        format_hms(self.duration)
    }
}

/// A summary of the most recent recorded gate run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: String,
    pub mode: String,
    pub scope: String,
    pub verdict: Verdict,
    pub steps: Vec<StepSummary>,
    /// Test counts, or `None` when the latest run has no populated counts.
    pub test_counts: Option<TestCounts>,
}

// === Reading ========================================================

/// Read the latest run from the history CSV at `path`. Returns `None` when the
/// file is absent or contains no data rows (so the caller emits the
/// "no prior gate run recorded" note).
pub fn latest_run_summary(path: &Path) -> Option<RunSummary> {
    let rows = read_rows(path)?;
    if rows.is_empty() {
        return None;
    }
    // File order is chronological; the last run_id is the most recent run.
    let last_run_id = rows.last()?.run_id.clone();
    let latest: Vec<&RawRow> = rows.iter().filter(|r| r.run_id == last_run_id).collect();
    if latest.is_empty() {
        return None;
    }

    let mode = latest[0].mode.clone();
    let scope = latest[0].scope.clone();

    // Re-derive the verdict with the same fold rule as Verdict::from_results:
    // any timeout dominates, then any issues, else clean. The synthetic
    // "overall" row carries "ok" and does not affect the fold.
    let verdict = fold_verdict(&latest);

    let mut steps = Vec::new();
    let mut test_counts = None;
    for r in &latest {
        if r.step == crate::history::OVERALL_STEP {
            continue;
        }
        steps.push(StepSummary {
            step: r.step.clone(),
            duration: Duration::from_secs_f64(r.duration_secs),
            status: r.status.clone(),
        });
        if r.step == "test" {
            if let (Some(total), Some(passed), Some(failed)) =
                (r.items_total, r.items_passed, r.items_failed)
            {
                test_counts = Some(TestCounts {
                    total,
                    passed,
                    failed,
                });
            }
        }
    }

    Some(RunSummary {
        run_id: last_run_id,
        mode,
        scope,
        verdict,
        steps,
        test_counts,
    })
}

fn fold_verdict(rows: &[&RawRow]) -> Verdict {
    if rows.iter().any(|r| r.status == Status::TimedOut.as_str()) {
        Verdict::Timeout
    } else if rows.iter().any(|r| r.status == Status::Issues.as_str()) {
        Verdict::Issues
    } else {
        Verdict::Clean
    }
}

/// A fully-parsed history row (every column the summary might use).
#[derive(Debug, Clone)]
struct RawRow {
    run_id: String,
    mode: String,
    scope: String,
    step: String,
    duration_secs: f64,
    status: String,
    items_total: Option<u64>,
    items_passed: Option<u64>,
    items_failed: Option<u64>,
}

/// Read all data rows, or `None` if the file cannot be opened.
fn read_rows(path: &Path) -> Option<Vec<RawRow>> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.ok()?;
        if i == 0 || line.trim().is_empty() {
            continue; // header or blank
        }
        if let Some(row) = parse_row(&line) {
            rows.push(row);
        }
    }
    Some(rows)
}

/// Parse one CSV line per the known column order:
/// `run_id,timestamp,mode,scope,step,duration_secs,status,warnings,errors,`
/// `items_total,items_passed,items_failed`.
fn parse_row(line: &str) -> Option<RawRow> {
    let f: Vec<&str> = line.split(',').collect();
    if f.len() < 7 {
        return None;
    }
    let opt_u64 = |s: &str| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            t.parse::<u64>().ok()
        }
    };
    Some(RawRow {
        run_id: f[0].to_string(),
        mode: f[2].to_string(),
        scope: f[3].to_string(),
        step: f[4].to_string(),
        duration_secs: f[5].trim().parse::<f64>().unwrap_or(0.0),
        status: f[6].to_string(),
        items_total: f.get(9).copied().and_then(opt_u64),
        items_passed: f.get(10).copied().and_then(opt_u64),
        items_failed: f.get(11).copied().and_then(opt_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::HEADER;

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "cargo-gate-summary-{name}-{}.csv",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&d);
        d
    }

    // Validates: Requirement 8.1, 8.2 -- the latest run_id with a CLEAN verdict
    // and known timings yields the expected summary.
    #[test]
    fn latest_clean_run_yields_expected_summary() {
        let path = tmp("clean");
        let body = format!(
            "{HEADER}\n\
             r1,2026-01-01 00:00:00,FULL,workspace,fmt,1.0,ok,0,0,,,\n\
             r1,2026-01-01 00:00:00,FULL,workspace,test,60.0,ok,0,0,100,100,0\n\
             r2,2026-01-02 00:00:00,FULL,workspace,fmt,2.0,ok,0,0,,,\n\
             r2,2026-01-02 00:00:00,FULL,workspace,clippy,3.0,ok,0,0,,,\n\
             r2,2026-01-02 00:00:00,FULL,workspace,test,120.0,ok,0,0,200,200,0\n\
             r2,2026-01-02 00:00:00,FULL,workspace,overall,130.0,ok,0,0,,,\n"
        );
        std::fs::write(&path, body).expect("write fixture");

        let summary = latest_run_summary(&path).expect("summary present");
        assert_eq!(summary.run_id, "r2");
        assert_eq!(summary.verdict, Verdict::Clean);
        // The overall row is excluded from the step list.
        assert_eq!(summary.steps.len(), 3);
        let test_step = summary
            .steps
            .iter()
            .find(|s| s.step == "test")
            .expect("test step");
        assert_eq!(test_step.duration_hms(), "0:02:00");
        assert_eq!(
            summary.test_counts,
            Some(TestCounts {
                total: 200,
                passed: 200,
                failed: 0
            })
        );
        let _ = std::fs::remove_file(&path);
    }

    // Validates: Requirement 8.3 -- absent history yields None.
    #[test]
    fn absent_history_yields_none() {
        let path = tmp("absent");
        let _ = std::fs::remove_file(&path);
        assert!(latest_run_summary(&path).is_none());
    }

    // Validates: Requirement 8.3 -- a header-only file yields None.
    #[test]
    fn empty_history_yields_none() {
        let path = tmp("empty");
        std::fs::write(&path, format!("{HEADER}\n")).expect("write header");
        assert!(latest_run_summary(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    // Validates: Requirement 8.4 -- missing test-count columns render as
    // unavailable rather than fabricated.
    #[test]
    fn missing_test_counts_render_as_unavailable() {
        let path = tmp("no-counts");
        let body = format!(
            "{HEADER}\n\
             r1,2026-01-01 00:00:00,FULL,workspace,fmt,1.0,ok,0,0,,,\n\
             r1,2026-01-01 00:00:00,FULL,workspace,test,60.0,ok,0,0,,,\n"
        );
        std::fs::write(&path, body).expect("write fixture");
        let summary = latest_run_summary(&path).expect("summary present");
        assert_eq!(summary.test_counts, None);
        let _ = std::fs::remove_file(&path);
    }
}
