//! Shared phase model: the identity of each gate step and the structured
//! result each step produces. Reporting, history, and the progress display all
//! consume these types.

use std::time::Duration;

/// The ordered steps of a gate run. `Build` and `Test` are the two halves of
/// the nextest invocation (compiling the test binaries vs running them), timed
/// separately so history can track each independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    Fmt,
    Clippy,
    Build,
    Test,
    /// The optional application build (`--build`), run after the test phase.
    /// Distinct from `Build` (which compiles the test binaries) so its very
    /// different duration trends independently in history and ETAs.
    AppBuild,
}

impl Step {
    /// Stable identifier used as the history CSV `step` column and as the
    /// progress-bar label. Must not change once history is accumulated.
    pub fn id(self) -> &'static str {
        match self {
            Step::Fmt => "fmt",
            Step::Clippy => "clippy",
            Step::Build => "build",
            Step::Test => "test",
            Step::AppBuild => "app-build",
        }
    }

    /// The steps in execution order.
    #[allow(dead_code)] // used in tests and useful as public API
    pub fn all() -> [Step; 5] {
        [
            Step::Fmt,
            Step::Clippy,
            Step::Build,
            Step::Test,
            Step::AppBuild,
        ]
    }
}

/// How a step finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Completed and clean (exit success, no errors/failures).
    Ok,
    /// Completed but produced errors, warnings-as-issues, or test failures.
    Issues,
    /// Terminated by the watchdog before finishing.
    TimedOut,
    /// Not run (e.g. a build error meant the test phase never started).
    Skipped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Issues => "issues",
            Status::TimedOut => "timed_out",
            Status::Skipped => "skipped",
        }
    }
}

/// Structured outcome of a single step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step: Step,
    pub status: Status,
    pub duration: Duration,
    /// Count of warnings surfaced by this step (clippy; 0 for others).
    pub warnings: u64,
    /// Count of errors surfaced by this step.
    pub errors: u64,
    /// Total work items (tests) known for this step, if applicable.
    pub items_total: Option<u64>,
    pub items_passed: Option<u64>,
    pub items_failed: Option<u64>,
    /// Lines destined for the review log (errors/warnings/failures). Empty for a
    /// clean step.
    pub review_lines: Vec<String>,
}

impl StepResult {
    /// A result for a step that was skipped (never executed).
    pub fn skipped(step: Step) -> Self {
        StepResult {
            step,
            status: Status::Skipped,
            duration: Duration::ZERO,
            warnings: 0,
            errors: 0,
            items_total: None,
            items_passed: None,
            items_failed: None,
            review_lines: Vec::new(),
        }
    }

    /// Convenience constructor for a freshly measured step with no items.
    pub fn new(step: Step, status: Status, duration: Duration) -> Self {
        StepResult {
            step,
            status,
            duration,
            warnings: 0,
            errors: 0,
            items_total: None,
            items_passed: None,
            items_failed: None,
            review_lines: Vec::new(),
        }
    }
}

/// The overall verdict for the whole run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    Issues,
    Timeout,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Clean => "CLEAN",
            Verdict::Issues => "ISSUES",
            Verdict::Timeout => "TIMEOUT",
        }
    }

    /// Process exit code for this verdict.
    pub fn exit_code(self) -> i32 {
        match self {
            Verdict::Clean => 0,
            Verdict::Issues => 1,
            Verdict::Timeout => 2,
        }
    }

    /// Fold a set of step results into one verdict. Any timeout dominates; then
    /// any issues; otherwise clean.
    pub fn from_results(results: &[StepResult]) -> Verdict {
        if results.iter().any(|r| r.status == Status::TimedOut) {
            Verdict::Timeout
        } else if results.iter().any(|r| matches!(r.status, Status::Issues)) {
            Verdict::Issues
        } else {
            Verdict::Clean
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_ids_are_stable_and_distinct() {
        let ids: Vec<&str> = Step::all().iter().map(|s| s.id()).collect();
        assert_eq!(ids, vec!["fmt", "clippy", "build", "test", "app-build"]);
    }

    #[test]
    fn app_build_id_is_distinct_from_test_binary_build() {
        // Validates: Requirement 15.5 -- the optional app build must be
        // distinguishable from the test-binary build in timing/history.
        assert_eq!(Step::AppBuild.id(), "app-build");
        assert_ne!(Step::AppBuild.id(), Step::Build.id());
    }

    #[test]
    fn verdict_timeout_dominates_issues() {
        let results = vec![
            StepResult::new(Step::Fmt, Status::Issues, Duration::from_secs(1)),
            StepResult::new(Step::Clippy, Status::TimedOut, Duration::from_secs(2)),
        ];
        assert_eq!(Verdict::from_results(&results), Verdict::Timeout);
    }

    #[test]
    fn verdict_issues_when_any_step_has_issues() {
        let results = vec![
            StepResult::new(Step::Fmt, Status::Ok, Duration::from_secs(1)),
            StepResult::new(Step::Clippy, Status::Issues, Duration::from_secs(2)),
        ];
        assert_eq!(Verdict::from_results(&results), Verdict::Issues);
    }

    #[test]
    fn verdict_clean_when_all_ok() {
        let results = vec![
            StepResult::new(Step::Fmt, Status::Ok, Duration::from_secs(1)),
            StepResult::new(Step::Test, Status::Ok, Duration::from_secs(2)),
        ];
        assert_eq!(Verdict::from_results(&results), Verdict::Clean);
    }

    #[test]
    fn exit_codes_match_verdicts() {
        assert_eq!(Verdict::Clean.exit_code(), 0);
        assert_eq!(Verdict::Issues.exit_code(), 1);
        assert_eq!(Verdict::Timeout.exit_code(), 2);
    }
}
