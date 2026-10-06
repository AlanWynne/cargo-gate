//! The run orchestrator: resolves scope, runs each phase in order with its live
//! bar and watchdog, splits the nextest run into build/test timing, writes all
//! artifacts, and computes the final verdict.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;

use crate::config::{BuildTarget, Config, Scope};
use crate::display::Display;
use crate::history;
use crate::phase::{Status, Step, StepResult, Verdict};
use crate::phases::fmt_clippy;
use crate::phases::nextest::{self, Runner};
use crate::report::{self, Section};
use crate::runner::run_with_capture;
use crate::workspace;

const ETA_WINDOW: usize = 10;

/// Paths to every artifact the run writes.
struct Artifacts {
    log_dir: PathBuf,
    raw_dir: PathBuf,
    report: PathBuf,
    review: PathBuf,
    history: PathBuf,
    timing: PathBuf,
}

impl Artifacts {
    fn new(log_dir: &Path) -> Self {
        let raw_dir = log_dir.join("raw");
        Artifacts {
            log_dir: log_dir.to_path_buf(),
            report: log_dir.join("gate.report.log"),
            review: log_dir.join("gate.review.log"),
            history: log_dir.join("gate.history.csv"),
            timing: log_dir.join("gate.timing.log"),
            raw_dir,
        }
    }

    fn raw(&self, step: &str, stream: &str) -> PathBuf {
        self.raw_dir.join(format!("{step}.{stream}.log"))
    }
}

/// Run the whole gate and return the process exit code.
pub fn run(config: &Config) -> Result<i32> {
    let cargo = cargo_bin();
    let cwd = std::env::current_dir()?;
    let root = workspace::workspace_root(&cargo, &cwd).unwrap_or(cwd.clone());

    let log_dir = if config.log_dir.is_absolute() {
        config.log_dir.clone()
    } else {
        root.join(&config.log_dir)
    };
    let artifacts = Artifacts::new(&log_dir);
    std::fs::create_dir_all(&artifacts.raw_dir).ok();

    let runner = detect_runner(&cargo, &root);
    let orphans = resolve_orphans(config, &cargo, &root);

    let quiet = !atty_stdout();
    let display = Display::new(quiet);

    let scope_label = config.scope.label();
    let mode = config.mode.as_str();

    let timeout = Duration::from_secs(config.step_timeout_secs);
    let overall_start = Instant::now();
    let start_wall = Local::now();
    // Headline ETA: prefer the median of past WHOLE-RUN durations (consistent
    // with the "actual" wall clock in the footer). Fall back to the rough
    // sum-of-phases estimate only until an "overall" row exists (first run after
    // upgrading), so variance stops reflecting the estimate-vs-wallclock gap.
    let overall_eta =
        history::overall_eta_total(&artifacts.history, mode, &scope_label, ETA_WINDOW).or_else(
            || {
                history::overall_eta(
                    &artifacts.history,
                    mode,
                    &scope_label,
                    ETA_WINDOW,
                    config.build.is_some(),
                )
            },
        );
    let start_header = start_header_line(start_wall, overall_eta);
    // Header first, then the mode/runner/scope banner (task 25.2 steps 1-2),
    // before any phase bar starts.
    display.println(&start_header);
    display.println(&format!(
        "cargo-gate: mode={} runner={} scope={}",
        mode,
        runner.label(),
        config.scope.describe()
    ));
    let mut results: Vec<StepResult> = Vec::new();

    // --- fmt --------------------------------------------------------------
    let eta = history::eta_for_step(
        &artifacts.history,
        Step::Fmt,
        mode,
        &scope_label,
        ETA_WINDOW,
    );
    display.start_spinner(Step::Fmt, eta);
    let fmt = fmt_clippy::run_fmt(
        &cargo,
        &root,
        &artifacts.raw("fmt", "stdout"),
        &artifacts.raw("fmt", "stderr"),
        timeout,
        || display.tick(Step::Fmt),
    )?;
    display.finish_phase(Step::Fmt, &phase_summary(&fmt));
    results.push(fmt);

    // --- clippy -----------------------------------------------------------
    let eta = history::eta_for_step(
        &artifacts.history,
        Step::Clippy,
        mode,
        &scope_label,
        ETA_WINDOW,
    );
    display.start_spinner(Step::Clippy, eta);
    let clippy = fmt_clippy::run_clippy(
        &cargo,
        &root,
        &config.scope,
        &artifacts.raw("clippy", "stdout"),
        &artifacts.raw("clippy", "stderr"),
        timeout,
        || display.tick(Step::Clippy),
    )?;
    display.finish_phase(Step::Clippy, &phase_summary(&clippy));
    results.push(clippy);

    // --- build + test (one nextest/cargo-test invocation, split timing) ---
    let (build_result, test_result) = run_test_phase(
        &cargo,
        &root,
        config,
        runner,
        &orphans,
        &artifacts,
        &display,
        mode,
        &scope_label,
        timeout,
    )?;
    results.push(build_result);
    results.push(test_result);

    // --- optional app build (--build) -------------------------------------
    // Runs after the test phase, before the verdict fold, so it lands in the
    // report, timing log, and history row and the verdict accounts for it.
    if let Some(target) = &config.build {
        let app = run_app_build_phase(
            &cargo,
            &root,
            config,
            &orphans,
            &artifacts,
            &display,
            mode,
            &scope_label,
            timeout,
            &results,
            target,
        )?;
        results.push(app);
    }

    let overall = overall_start.elapsed();
    let verdict = Verdict::from_results(&results);

    let end_wall = Local::now();
    let completion_line = completion_footer_line(end_wall, overall, overall_eta);

    write_all_artifacts(
        &artifacts,
        config,
        runner,
        &results,
        verdict,
        overall,
        mode,
        &scope_label,
        &start_header,
        &completion_line,
    )?;

    // All phase bars are finished at this point; make sure they are flushed so
    // the trailing footer and verdict do not race the live redraw. Then print
    // the completion/variance footer (step 4) and the final verdict (step 5).
    display.finish_all();
    display.println(&completion_line);
    print_final_summary(&display, &artifacts, &results, verdict, overall);
    Ok(verdict.exit_code())
}

/// Run the test phase: optionally list first for the total, then run, measuring
/// build time (until the first test starts) and test time separately.
#[allow(clippy::too_many_arguments)]
fn run_test_phase(
    cargo: &str,
    root: &Path,
    config: &Config,
    runner: Runner,
    orphans: &[String],
    artifacts: &Artifacts,
    display: &Display,
    mode: &str,
    scope_label: &str,
    timeout: Duration,
) -> Result<(StepResult, StepResult)> {
    // Total up front via `nextest list` (stable JSON) when using nextest.
    let known_total = if runner == Runner::Nextest {
        list_total(cargo, root, &config.scope, orphans, artifacts)
            .ok()
            .flatten()
    } else {
        None
    };

    let build_eta = history::eta_for_step(
        &artifacts.history,
        Step::Build,
        mode,
        scope_label,
        ETA_WINDOW,
    );
    display.start_spinner(Step::Build, build_eta);

    let (args, env) = match runner {
        Runner::Nextest => (
            nextest::nextest_run_args(&config.scope, orphans),
            vec![(
                "NEXTEST_EXPERIMENTAL_LIBTEST_JSON".to_string(),
                "1".to_string(),
            )],
        ),
        Runner::CargoTest => (nextest::cargo_test_args(&config.scope, orphans), vec![]),
    };

    let stdout_log = artifacts.raw("test", "stdout");
    let stderr_log = artifacts.raw("test", "stderr");

    // Progress state shared across the capture callback.
    let progress = Cell::new(nextest::TestProgress {
        total: known_total,
        done: 0,
        build_finished: false,
    });
    let build_done_at: Cell<Option<Instant>> = Cell::new(None);
    let test_started = Instant::now();
    let build_transitioned = Cell::new(false);
    // Track the last total pushed to the display so we only update the test bar
    // length when the value actually changes (task 24.2).
    let last_total: Cell<Option<u64>> = Cell::new(None);

    let out = run_with_capture(
        cargo,
        &args,
        root,
        &stdout_log,
        &stderr_log,
        &env,
        timeout,
        |line| {
            let mut p = progress.take();
            let advanced = nextest::ingest_line(line, &mut p);
            // First time we know the build is done, finish the build bar and
            // flip the test bar to a count.
            if p.build_finished && !build_transitioned.get() {
                build_transitioned.set(true);
                build_done_at.set(Some(Instant::now()));
                let build_elapsed = test_started.elapsed();
                display.finish_phase(Step::Build, &report::format_hms(build_elapsed));
                let test_eta = history::eta_for_step(
                    &artifacts.history,
                    Step::Test,
                    mode,
                    scope_label,
                    ETA_WINDOW,
                );
                display.start_spinner(Step::Test, test_eta);
                // One-time switch to a determinate done/total bar.
                display.start_test(p.total.unwrap_or(0));
                last_total.set(p.total);
            }
            if advanced {
                // Only touch the length when the known total actually changed,
                // and never reset the position (task 24.1/24.2).
                if p.total != last_total.get() {
                    if let Some(t) = p.total {
                        display.set_test_total(t);
                    }
                    last_total.set(p.total);
                }
                display.set_test_done(p.done);
            } else if !build_transitioned.get() {
                // Only tick the build bar while still in the build phase
                // (task 24.3).
                display.tick(Step::Build);
            }
            progress.set(p);
        },
    )?;

    let total_elapsed = test_started.elapsed();
    let build_elapsed = build_done_at
        .get()
        .map(|t| t.duration_since(test_started))
        .unwrap_or(total_elapsed);
    let run_elapsed = total_elapsed.saturating_sub(build_elapsed);

    // If we never transitioned (build error), the build bar is still spinning.
    if !build_transitioned.get() {
        display.finish_phase(
            Step::Build,
            &format!("{} (no tests ran)", report::format_hms(build_elapsed)),
        );
    }

    let final_progress = progress.take();

    // Parse final totals from the stable human summary.
    let combined_text = read_both(&stdout_log, &stderr_log);
    let totals = match runner {
        Runner::Nextest => nextest::parse_nextest_summary(&combined_text),
        Runner::CargoTest => nextest::parse_cargo_test_summary(&combined_text),
    };

    let timed_out = out.timed_out;
    // The build "failed before tests" when the run never transitioned to the
    // test phase and exited unsuccessfully with no parsed test totals.
    let build_failed =
        !build_transitioned.get() && !out.success() && totals.is_none() && !timed_out;

    let build_status = if timed_out && !build_transitioned.get() {
        Status::TimedOut
    } else if build_failed {
        Status::Issues
    } else {
        Status::Ok
    };

    let mut build_result = StepResult::new(Step::Build, build_status, build_elapsed);
    if build_status == Status::Issues {
        build_result.errors = 1;
        build_result
            .review_lines
            .push("build: test binaries failed to compile (see raw/test.stderr.log)".to_string());
    }

    // When the build failed, the tests never ran: the test step is Skipped, not
    // falsely Ok.
    let test_result = if build_failed {
        display.finish_phase(Step::Test, "skipped (build failed)");
        StepResult::skipped(Step::Test)
    } else {
        let (test_status, review_lines) =
            classify_test(out.success(), timed_out, &totals, &combined_text);
        display.finish_phase(
            Step::Test,
            &test_phase_summary(&totals, run_elapsed, test_status),
        );
        let mut r = StepResult::new(Step::Test, test_status, run_elapsed);
        r.review_lines = review_lines;
        if let Some(t) = &totals {
            r.items_total = Some(t.run);
            r.items_passed = Some(t.passed);
            r.items_failed = Some(t.failed);
        } else if let Some(t) = final_progress.total {
            r.items_total = Some(t);
        }
        r
    };

    Ok((build_result, test_result))
}

/// Whether the optional build phase must be skipped because the gate so far is
/// not clean (Requirement 15.4): any issue or timeout among the prior steps.
fn app_build_should_skip(results: &[StepResult]) -> bool {
    matches!(
        Verdict::from_results(results),
        Verdict::Issues | Verdict::Timeout
    )
}

/// Run the optional post-test application build as a distinct, timed phase
/// (Requirement 15). Skips (without running cargo) when the gate is not clean.
#[allow(clippy::too_many_arguments)]
fn run_app_build_phase(
    cargo: &str,
    root: &Path,
    config: &Config,
    orphans: &[String],
    artifacts: &Artifacts,
    display: &Display,
    mode: &str,
    scope_label: &str,
    timeout: Duration,
    results_so_far: &[StepResult],
    target: &BuildTarget,
) -> Result<StepResult> {
    // Req 15.4: do not build from a failing gate.
    if app_build_should_skip(results_so_far) {
        display.finish_phase(Step::AppBuild, "skipped (gate not clean)");
        return Ok(StepResult::skipped(Step::AppBuild));
    }

    let eta = history::eta_for_step(
        &artifacts.history,
        Step::AppBuild,
        mode,
        scope_label,
        ETA_WINDOW,
    );
    display.start_spinner(Step::AppBuild, eta);
    let args = nextest::cargo_build_args(&config.scope, orphans, target, config.release);
    let started = Instant::now();
    let out = run_with_capture(
        cargo,
        &args,
        root,
        &artifacts.raw("app-build", "stdout"),
        &artifacts.raw("app-build", "stderr"),
        &[],
        timeout,
        |_| display.tick(Step::AppBuild),
    )?;
    let elapsed = started.elapsed();

    let status = if out.timed_out {
        Status::TimedOut
    } else if !out.success() {
        Status::Issues
    } else {
        Status::Ok
    };
    let mut result = StepResult::new(Step::AppBuild, status, elapsed);
    if status == Status::Issues {
        result.errors = 1;
        result
            .review_lines
            .push("app-build: cargo build failed (see raw/app-build.stderr.log)".to_string());
    }
    display.finish_phase(Step::AppBuild, &phase_summary(&result));
    Ok(result)
}

/// Run `nextest list` to get the total test count up front.
fn list_total(
    cargo: &str,
    root: &Path,
    scope: &Scope,
    orphans: &[String],
    artifacts: &Artifacts,
) -> Result<Option<u64>> {
    let args = nextest::nextest_list_args(scope, orphans);
    let stdout_log = artifacts.raw("list", "stdout");
    let stderr_log = artifacts.raw("list", "stderr");
    let out = run_with_capture(
        cargo,
        &args,
        root,
        &stdout_log,
        &stderr_log,
        &[],
        Duration::from_secs(600),
        |_| {},
    )?;
    if !out.success() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&stdout_log).unwrap_or_default();
    Ok(nextest::parse_total_from_list(&json))
}

/// Classify the test step outcome and collect review lines on failure.
fn classify_test(
    success: bool,
    timed_out: bool,
    totals: &Option<nextest::TestTotals>,
    combined_text: &str,
) -> (Status, Vec<String>) {
    if timed_out {
        return (
            Status::TimedOut,
            vec!["test: watchdog terminated the run".to_string()],
        );
    }
    let failed = totals.as_ref().map(|t| t.failed).unwrap_or(0);
    if success && failed == 0 {
        return (Status::Ok, Vec::new());
    }
    // Collect FAIL / failed lines for the review log.
    let mut lines: Vec<String> = combined_text
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with("FAIL [")
                || t.contains("FAILED")
                || (t.contains("failed") && !t.contains("0 failed"))
        })
        .map(|l| l.trim().to_string())
        .collect();
    lines.dedup();
    if lines.is_empty() {
        lines.push("test: run reported failure (see raw/test.stderr.log)".to_string());
    }
    (Status::Issues, lines)
}

/// Build the run-start header line: start wall-clock time and, when history
/// allows, the estimated completion time and ETA duration.
fn start_header_line(start_wall: chrono::DateTime<Local>, eta: Option<Duration>) -> String {
    let start_str = start_wall.format("%Y-%m-%d %H:%M:%S").to_string();
    match eta {
        Some(d) => {
            let completion = start_wall + chrono::Duration::seconds(d.as_secs() as i64);
            let completion_str = completion.format("%H:%M:%S").to_string();
            format!(
                "started {}   estimated completion ~{} (ETA {} from history)",
                start_str,
                completion_str,
                report::format_hms(d)
            )
        }
        None => format!(
            "started {}   estimated completion: unknown (no history yet)",
            start_str
        ),
    }
}

/// Build the run-end completion line: completion wall-clock time, actual total
/// duration, and -- only when an overall ETA was available -- the variance as a
/// signed absolute duration and percentage.
fn completion_footer_line(
    end_wall: chrono::DateTime<Local>,
    overall: Duration,
    eta: Option<Duration>,
) -> String {
    let end_str = end_wall.format("%Y-%m-%d %H:%M:%S").to_string();
    let base = format!(
        "completed {}   actual {}",
        end_str,
        report::format_hms(overall)
    );
    match eta {
        Some(d) => format!(
            "{}   ETA was {}   variance {}",
            base,
            report::format_hms(d),
            report::format_variance(overall, d)
        ),
        None => base,
    }
}

#[allow(clippy::too_many_arguments)]
fn write_all_artifacts(
    artifacts: &Artifacts,
    config: &Config,
    runner: Runner,
    results: &[StepResult],
    verdict: Verdict,
    overall: Duration,
    mode: &str,
    scope_label: &str,
    start_header: &str,
    completion_line: &str,
) -> Result<()> {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let run_id = uuid::Uuid::new_v4().to_string();

    let header = format!(
        "{}\ncargo-gate report -- {}  mode={}  runner={}  scope={}",
        start_header,
        timestamp,
        mode,
        runner.label(),
        config.scope.describe()
    );

    let fmt_stderr = artifacts.raw("fmt", "stderr");
    let clippy_stdout = artifacts.raw("clippy", "stdout");
    let clippy_stderr = artifacts.raw("clippy", "stderr");
    let test_stdout = artifacts.raw("test", "stdout");
    let test_stderr = artifacts.raw("test", "stderr");
    let app_build_stderr = artifacts.raw("app-build", "stderr");
    let mut sections = vec![
        Section {
            title: "fmt (stderr)",
            raw_log: &fmt_stderr,
        },
        Section {
            title: "clippy (stdout)",
            raw_log: &clippy_stdout,
        },
        Section {
            title: "clippy (stderr)",
            raw_log: &clippy_stderr,
        },
        Section {
            title: "test (stdout)",
            raw_log: &test_stdout,
        },
        Section {
            title: "test (stderr)",
            raw_log: &test_stderr,
        },
    ];
    // Only include the app-build section when the optional build phase ran, so a
    // default (no --build) report is unchanged.
    if results.iter().any(|r| r.step == Step::AppBuild) {
        sections.push(Section {
            title: "app-build (stderr)",
            raw_log: &app_build_stderr,
        });
    }

    let count_summary = test_count_summary(results);
    let detail = format!(
        "tests: {}   elapsed: {}\n{}",
        count_summary,
        report::format_hms(overall),
        completion_line
    );

    report::write_combined_report(&artifacts.report, &header, &sections, verdict, &detail)?;
    report::write_review_log(&artifacts.review, results)?;
    report::write_timing_log(
        &artifacts.timing,
        &format!("cargo-gate timing -- {timestamp}  mode={mode}  scope={scope_label}"),
        results,
        overall,
        &format!("tests: {count_summary}"),
        completion_line,
    )?;
    history::append_rows(
        &artifacts.history,
        &run_id,
        &timestamp,
        mode,
        scope_label,
        results,
    )?;
    // Record the whole-run wall-clock time so the headline ETA is learned from
    // past TOTAL durations (which include the list pre-pass and overhead the
    // per-phase sum misses), keeping the ETA and the "actual" footer consistent.
    history::append_overall_row(
        &artifacts.history,
        &run_id,
        &timestamp,
        mode,
        scope_label,
        overall,
    )?;
    Ok(())
}

fn print_final_summary(
    display: &Display,
    artifacts: &Artifacts,
    results: &[StepResult],
    verdict: Verdict,
    overall: Duration,
) {
    let count_summary = test_count_summary(results);
    display.println("");
    match verdict {
        Verdict::Clean => display.println(&format!(
            "cargo-gate: CLEAN -- {} in {}. See {}/gate.report.log",
            count_summary,
            report::format_hms(overall),
            artifacts.log_dir.display()
        )),
        Verdict::Issues => display.println(&format!(
            "cargo-gate: ISSUES -- see {}/gate.review.log ({})",
            artifacts.log_dir.display(),
            count_summary
        )),
        Verdict::Timeout => display.println(&format!(
            "cargo-gate: TIMEOUT -- a step exceeded the watchdog. See {}/gate.report.log",
            artifacts.log_dir.display()
        )),
    }
}

fn test_count_summary(results: &[StepResult]) -> String {
    let test = results.iter().find(|r| r.step == Step::Test);
    match test.and_then(|t| t.items_total.map(|total| (total, t))) {
        Some((total, t)) => format!(
            "{} run, {} passed, {} failed",
            total,
            t.items_passed.unwrap_or(0),
            t.items_failed.unwrap_or(0)
        ),
        None => "counts unavailable".to_string(),
    }
}

fn phase_summary(r: &StepResult) -> String {
    let base = report::format_hms(r.duration);
    match r.step {
        Step::Clippy => {
            if r.warnings > 0 || r.errors > 0 {
                format!("{base}  {} warnings, {} errors", r.warnings, r.errors)
            } else {
                format!("{base}  clean")
            }
        }
        _ => match r.status {
            Status::Ok => format!("{base}  ok"),
            Status::Issues => format!("{base}  issues"),
            Status::TimedOut => format!("{base}  TIMED OUT"),
            Status::Skipped => "skipped".to_string(),
        },
    }
}

fn test_phase_summary(
    totals: &Option<nextest::TestTotals>,
    elapsed: Duration,
    status: Status,
) -> String {
    let base = report::format_hms(elapsed);
    match totals {
        Some(t) if status == Status::Ok => format!("{}/{} passed  {base}", t.passed, t.run),
        Some(t) => format!("{} passed, {} FAILED  {base}", t.passed, t.failed),
        None if status == Status::TimedOut => format!("TIMED OUT  {base}"),
        None => format!("done  {base}"),
    }
}

fn read_both(stdout_log: &Path, stderr_log: &Path) -> String {
    let mut s = std::fs::read_to_string(stderr_log).unwrap_or_default();
    s.push('\n');
    s.push_str(&std::fs::read_to_string(stdout_log).unwrap_or_default());
    s
}

fn resolve_orphans(config: &Config, cargo: &str, root: &Path) -> Vec<String> {
    if let Scope::AppOnly { closure_of } = &config.scope {
        let orphans = workspace::derive_orphans(cargo, root, closure_of);
        if orphans.is_empty() {
            eprintln!(
                "cargo-gate: --app-only could not derive the orphan list; running full --workspace."
            );
        }
        orphans
    } else {
        Vec::new()
    }
}

fn detect_runner(cargo: &str, root: &Path) -> Runner {
    let ok = std::process::Command::new(cargo)
        .args(["nextest", "--version"])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Runner::Nextest
    } else {
        Runner::CargoTest
    }
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Best-effort TTY detection without an extra dependency.
fn atty_stdout() -> bool {
    #[cfg(windows)]
    {
        // On Windows, assume a TTY unless output is redirected. indicatif also
        // guards itself; this is just to pick the quiet code path for CI.
        use std::io::IsTerminal;
        std::io::stdout().is_terminal()
    }
    #[cfg(not(windows))]
    {
        use std::io::IsTerminal;
        std::io::stdout().is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_summary_reports_unavailable_without_totals() {
        let results = vec![StepResult::new(Step::Test, Status::Ok, Duration::ZERO)];
        assert_eq!(test_count_summary(&results), "counts unavailable");
    }

    #[test]
    fn test_count_summary_formats_totals() {
        let mut t = StepResult::new(Step::Test, Status::Ok, Duration::ZERO);
        t.items_total = Some(100);
        t.items_passed = Some(98);
        t.items_failed = Some(2);
        assert_eq!(test_count_summary(&[t]), "100 run, 98 passed, 2 failed");
    }

    #[test]
    fn classify_test_ok_when_success_and_no_failures() {
        let totals = Some(nextest::TestTotals {
            run: 10,
            passed: 10,
            failed: 0,
        });
        let (status, lines) = classify_test(true, false, &totals, "");
        assert_eq!(status, Status::Ok);
        assert!(lines.is_empty());
    }

    #[test]
    fn classify_test_issues_collects_fail_lines() {
        let totals = Some(nextest::TestTotals {
            run: 10,
            passed: 9,
            failed: 1,
        });
        let text = "        FAIL [   0.01s] crate module::test_name\nSummary: 9 passed, 1 failed";
        let (status, lines) = classify_test(false, false, &totals, text);
        assert_eq!(status, Status::Issues);
        assert!(lines.iter().any(|l| l.contains("FAIL [")));
    }

    #[test]
    fn classify_test_timeout() {
        let (status, lines) = classify_test(false, true, &None, "");
        assert_eq!(status, Status::TimedOut);
        assert!(!lines.is_empty());
    }

    #[test]
    fn phase_summary_clippy_reports_warnings() {
        let mut c = StepResult::new(Step::Clippy, Status::Ok, Duration::from_secs(5));
        c.warnings = 3;
        assert!(phase_summary(&c).contains("3 warnings"));
    }

    #[test]
    fn app_build_skipped_when_a_prior_step_has_issues() {
        // Validates: Requirement 15.4 -- skip the build phase from a failing gate.
        let results = vec![
            StepResult::new(Step::Fmt, Status::Ok, Duration::ZERO),
            StepResult::new(Step::Clippy, Status::Issues, Duration::ZERO),
        ];
        assert!(app_build_should_skip(&results));
    }

    #[test]
    fn app_build_skipped_when_a_prior_step_timed_out() {
        // Validates: Requirement 15.4 -- a timeout also blocks the build phase.
        let results = vec![StepResult::new(
            Step::Test,
            Status::TimedOut,
            Duration::ZERO,
        )];
        assert!(app_build_should_skip(&results));
    }

    #[test]
    fn app_build_runs_when_gate_is_clean() {
        // Validates: Requirement 15.2 -- a clean gate allows the build phase.
        let results = vec![
            StepResult::new(Step::Fmt, Status::Ok, Duration::ZERO),
            StepResult::new(Step::Clippy, Status::Ok, Duration::ZERO),
            StepResult::new(Step::Build, Status::Ok, Duration::ZERO),
            StepResult::new(Step::Test, Status::Ok, Duration::ZERO),
        ];
        assert!(!app_build_should_skip(&results));
    }

    #[test]
    fn app_build_issue_folds_into_verdict() {
        // Validates: Requirement 15.7 -- a build-phase failure folds to ISSUES.
        let results = vec![
            StepResult::new(Step::Fmt, Status::Ok, Duration::ZERO),
            StepResult::new(Step::Test, Status::Ok, Duration::ZERO),
            StepResult::new(Step::AppBuild, Status::Issues, Duration::ZERO),
        ];
        assert_eq!(Verdict::from_results(&results), Verdict::Issues);
        assert_eq!(Verdict::from_results(&results).exit_code(), 1);
    }
}
