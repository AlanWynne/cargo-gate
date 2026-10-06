//! Durable log artifacts: the combined filtered report, the review log (empty
//! == clean), and the timing log.
//!
//! The combined report keeps the signal and drops the noise, matching the
//! ffwb-gate intent: per-test success lines and blank lines are filtered out;
//! section headers, group/summary outcomes, warnings, and errors are kept.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::phase::{StepResult, Verdict};

/// A raw log section to fold into the combined report.
pub struct Section<'a> {
    pub title: &'a str,
    pub raw_log: &'a Path,
}

/// Return true if a line is noise that the combined report should drop:
/// per-test "... ok" lines, nextest "PASS [" lines, and blank lines.
fn is_noise(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    if t.ends_with("... ok") {
        return true;
    }
    if t.starts_with("PASS [") || t.contains("        PASS [") {
        return true;
    }
    false
}

/// Write the combined, filtered report from the given sections plus a header and
/// the final verdict block.
pub fn write_combined_report(
    path: &Path,
    header: &str,
    sections: &[Section<'_>],
    verdict: Verdict,
    verdict_detail: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = File::create(path).with_context(|| format!("create {path:?}"))?;
    writeln!(file, "{header}")?;
    for section in sections {
        writeln!(file, "===== {} =====", section.title)?;
        match std::fs::read_to_string(section.raw_log) {
            Ok(text) => {
                for line in text.lines() {
                    if !is_noise(line) {
                        writeln!(file, "{line}")?;
                    }
                }
            }
            Err(_) => {
                writeln!(file, "(no output captured)")?;
            }
        }
    }
    writeln!(file)?;
    writeln!(file, "===== VERDICT =====")?;
    writeln!(file, "verdict: {}   {}", verdict.as_str(), verdict_detail)?;
    Ok(())
}

/// Write the review log: the concatenation of every step's review_lines. An
/// empty file means a clean gate.
pub fn write_review_log(path: &Path, results: &[StepResult]) -> Result<usize> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = File::create(path).with_context(|| format!("create {path:?}"))?;
    let mut count = 0;
    for r in results {
        for line in &r.review_lines {
            writeln!(file, "[{}] {}", r.step.id(), line)?;
            count += 1;
        }
    }
    Ok(count)
}

/// Write the per-step timing log. `completion_line` is the overall
/// completion/variance line appended to the footer (may be empty).
pub fn write_timing_log(
    path: &Path,
    header: &str,
    results: &[StepResult],
    total: Duration,
    summary: &str,
    completion_line: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = File::create(path).with_context(|| format!("create {path:?}"))?;
    writeln!(file, "{header}")?;
    for r in results {
        writeln!(
            file,
            "{:<10} {:>8.1}s   {}",
            r.step.id(),
            r.duration.as_secs_f64(),
            r.status.as_str()
        )?;
    }
    writeln!(file)?;
    writeln!(file, "{summary}")?;
    writeln!(file, "{:<10} {:>8.1}s", "TOTAL", total.as_secs_f64())?;
    if !completion_line.is_empty() {
        writeln!(file, "{completion_line}")?;
    }
    Ok(())
}

/// Format the variance of an actual duration against an ETA as a signed
/// absolute duration and a signed percentage, e.g. `+0:00:13 (+4.1%)` or
/// `-0:00:05 (-1.6%)`. Positive means slower than estimated. When `eta` is
/// zero the percentage is unavailable (no divide-by-zero); the absolute delta
/// is still shown as `(n/a)`.
pub fn format_variance(actual: Duration, eta: Duration) -> String {
    let slower = actual >= eta;
    let sign = if slower { "+" } else { "-" };
    let abs_delta = if slower { actual - eta } else { eta - actual };
    let abs_hms = format_hms(abs_delta);
    if eta.is_zero() {
        return format!("{sign}{abs_hms} (n/a)");
    }
    let pct = (actual.as_secs_f64() - eta.as_secs_f64()) / eta.as_secs_f64() * 100.0;
    let pct_sign = if pct >= 0.0 { "+" } else { "-" };
    format!("{sign}{abs_hms} ({pct_sign}{:.1}%)", pct.abs())
}

/// Format a Duration as H:MM:SS for human display.
pub fn format_hms(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::{Status, Step, StepResult};

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("cargo-gate-report-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn noise_filter_drops_passing_tests_and_blanks() {
        assert!(is_noise(""));
        assert!(is_noise("   "));
        assert!(is_noise("test foo::bar ... ok"));
        assert!(is_noise("        PASS [   0.012s] crate test::name"));
        assert!(!is_noise("error[E0425]: cannot find value"));
        assert!(!is_noise("warning: unused import"));
        assert!(!is_noise("test result: ok. 10 passed; 0 failed"));
    }

    #[test]
    fn combined_report_filters_noise_and_keeps_signal() {
        let dir = tmp("combined");
        let raw = dir.join("raw.log");
        std::fs::write(
            &raw,
            "Compiling foo\ntest a ... ok\n\nwarning: x unused\n        PASS [ 0.01s] b\nerror: boom\n",
        )
        .unwrap();
        let out = dir.join("report.log");
        write_combined_report(
            &out,
            "cargo-gate report",
            &[Section {
                title: "test",
                raw_log: &raw,
            }],
            Verdict::Issues,
            "tests: 1 run",
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("Compiling foo"));
        assert!(text.contains("warning: x unused"));
        assert!(text.contains("error: boom"));
        assert!(!text.contains("... ok"));
        assert!(!text.contains("PASS ["));
        assert!(text.contains("verdict: ISSUES"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_log_empty_when_all_clean() {
        let dir = tmp("review-clean");
        let out = dir.join("review.log");
        let results = vec![StepResult::new(Step::Fmt, Status::Ok, Duration::ZERO)];
        let count = write_review_log(&out, &results).unwrap();
        assert_eq!(count, 0);
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_log_lists_issue_lines_with_step_tag() {
        let dir = tmp("review-issues");
        let out = dir.join("review.log");
        let mut clippy = StepResult::new(Step::Clippy, Status::Issues, Duration::ZERO);
        clippy.review_lines = vec!["warning: unused".to_string(), "error: boom".to_string()];
        let count = write_review_log(&out, &[clippy]).unwrap();
        assert_eq!(count, 2);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("[clippy] warning: unused"));
        assert!(text.contains("[clippy] error: boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timing_log_includes_total_and_completion_line() {
        let dir = tmp("timing");
        let out = dir.join("timing.log");
        let results = vec![StepResult::new(
            Step::Fmt,
            Status::Ok,
            Duration::from_secs(13),
        )];
        write_timing_log(
            &out,
            "cargo-gate timing -- test",
            &results,
            Duration::from_secs(326),
            "tests: 10 run",
            "completed 2026-10-05 19:50:14   actual 0:05:26   ETA was 0:05:13   variance +0:00:13 (+4.2%)",
        )
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("TOTAL"));
        assert!(text.contains("completed 2026-10-05 19:50:14"));
        assert!(text.contains("variance +0:00:13 (+4.2%)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_variance_positive_when_slower() {
        // 13/313*100 = 4.1533..., rounded to one decimal via {:.1} = 4.2.
        let v = format_variance(Duration::from_secs(326), Duration::from_secs(313));
        assert_eq!(v, "+0:00:13 (+4.2%)");
    }

    #[test]
    fn format_variance_negative_when_faster() {
        let v = format_variance(Duration::from_secs(308), Duration::from_secs(313));
        assert_eq!(v, "-0:00:05 (-1.6%)");
    }

    #[test]
    fn format_variance_zero_eta_marks_percentage_unavailable() {
        let v = format_variance(Duration::from_secs(13), Duration::ZERO);
        assert_eq!(v, "+0:00:13 (n/a)");
    }

    #[test]
    fn format_hms_formats_correctly() {
        assert_eq!(format_hms(Duration::from_secs(0)), "0:00:00");
        assert_eq!(format_hms(Duration::from_secs(65)), "0:01:05");
        assert_eq!(format_hms(Duration::from_secs(3661)), "1:01:01");
    }
}
