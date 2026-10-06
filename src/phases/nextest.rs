//! The test phase, split into `build` (compiling the test binaries) and `test`
//! (running them), driven by cargo-nextest when present and `cargo test` as a
//! fallback.
//!
//! ## Getting a live `done/total` count
//!
//! 1. **Total up front (stable):** `cargo nextest list --message-format json`
//!    reports every test without running anything. We sum `test_count` across
//!    binaries so the progress bar knows N before the run starts.
//! 2. **Live increments (experimental, with fallback):** during the run we parse
//!    nextest's libtest-json event stream (gated behind
//!    `NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1`). If a line does not parse as a JSON
//!    event we fall back to nextest's human `(n/N)` progress marker via regex.
//!    Either way the bar advances; the experimental format is never the sole
//!    source of truth.
//!
//! The final pass/fail counts are taken from nextest's `Summary [...]` line (or
//! `cargo test`'s `test result:` lines) which is stable human output.

use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

use crate::config::{BuildTarget, Scope};

/// Which test runner is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Nextest,
    CargoTest,
}

impl Runner {
    pub fn label(self) -> &'static str {
        match self {
            Runner::Nextest => "nextest",
            Runner::CargoTest => "cargo-test",
        }
    }
}

/// Translate a scope into the package-selection args shared by list and run.
pub fn scope_args(scope: &Scope, orphans: &[String]) -> Vec<String> {
    match scope {
        Scope::Crate { name } => vec!["-p".to_string(), name.clone()],
        Scope::Workspace => vec!["--workspace".to_string()],
        Scope::AppOnly { .. } => {
            let mut v = vec!["--workspace".to_string()];
            for o in orphans {
                v.push("--exclude".to_string());
                v.push(o.clone());
            }
            v
        }
    }
}

/// Args for `cargo nextest list --message-format json <scope>`.
pub fn nextest_list_args(scope: &Scope, orphans: &[String]) -> Vec<String> {
    let mut args = vec![
        "nextest".to_string(),
        "list".to_string(),
        "--message-format".to_string(),
        "json".to_string(),
    ];
    args.extend(scope_args(scope, orphans));
    args
}

/// Args for `cargo nextest run <scope>` with the experimental libtest-json
/// message format for live events.
pub fn nextest_run_args(scope: &Scope, orphans: &[String]) -> Vec<String> {
    let mut args = vec![
        "nextest".to_string(),
        "run".to_string(),
        "--message-format".to_string(),
        "libtest-json".to_string(),
    ];
    args.extend(scope_args(scope, orphans));
    args
}

/// Args for the `cargo test` fallback.
pub fn cargo_test_args(scope: &Scope, orphans: &[String]) -> Vec<String> {
    let mut args = vec!["test".to_string()];
    args.extend(scope_args(scope, orphans));
    args
}

/// Args for the optional post-test build phase: `cargo build <selection>
/// [--release]`. `BuildTarget::Scope` reuses the active scope selection (so
/// `--build` honours `--crate` / `--app-only`); `BuildTarget::Bin(name)` builds
/// just `-p <name>`.
pub fn cargo_build_args(
    scope: &Scope,
    orphans: &[String],
    target: &BuildTarget,
    release: bool,
) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    match target {
        BuildTarget::Scope => args.extend(scope_args(scope, orphans)),
        BuildTarget::Bin(name) => {
            args.push("-p".to_string());
            args.push(name.clone());
        }
    }
    if release {
        args.push("--release".to_string());
    }
    args
}

/// The build vs test split: nextest prints `Starting N tests` once compilation
/// is done, so the first time we see a total/started marker we treat the build
/// phase as finished and the test phase as begun.
#[derive(Debug, Default)]
pub struct TestProgress {
    pub total: Option<u64>,
    pub done: u64,
    pub build_finished: bool,
}

/// The structure of a `cargo nextest list --message-format json` document that
/// we care about: a map of binary-id to an object carrying `test-count`.
///
/// nextest's list JSON nests test counts under `"rust-suites"`; rather than
/// model the whole schema we count `"test-count"` occurrences robustly by
/// summing every `test-count` field found. A tolerant parse keeps us resilient
/// to schema drift.
#[derive(Debug, Deserialize)]
struct ListDoc {
    #[serde(rename = "rust-suites")]
    rust_suites: Option<std::collections::HashMap<String, SuiteEntry>>,
}

#[derive(Debug, Deserialize)]
struct SuiteEntry {
    #[serde(rename = "test-count")]
    test_count: Option<u64>,
    #[serde(rename = "testcases")]
    testcases: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Sum the total test count from a nextest list JSON document. Falls back to
/// counting testcase entries when `test-count` is absent.
pub fn parse_total_from_list(json: &str) -> Option<u64> {
    let doc: ListDoc = serde_json::from_str(json).ok()?;
    let suites = doc.rust_suites?;
    let mut total = 0u64;
    for entry in suites.values() {
        if let Some(c) = entry.test_count {
            total += c;
        } else if let Some(cases) = &entry.testcases {
            total += cases.len() as u64;
        }
    }
    Some(total)
}

/// A single libtest-json event line we care about during the run.
#[derive(Debug, Deserialize)]
struct LibtestEvent {
    #[serde(rename = "type")]
    kind: String,
    event: Option<String>,
    test_count: Option<u64>,
}

/// Update progress from one live stderr/stdout line. Returns true if the line
/// advanced the count (so the caller can refresh the bar). Tolerant: tries the
/// JSON event first, then the human `(n/N)` regex.
pub fn ingest_line(line: &str, progress: &mut TestProgress) -> bool {
    let trimmed = line.trim();

    // 1. Experimental libtest-json event.
    if trimmed.starts_with('{') {
        if let Ok(ev) = serde_json::from_str::<LibtestEvent>(trimmed) {
            return apply_event(&ev, progress);
        }
    }

    // 2. Human fallbacks.
    if let Some(caps) = starting_re().captures(trimmed) {
        if let Some(n) = caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok()) {
            progress.total = Some(n);
            progress.build_finished = true;
            return true;
        }
    }
    if let Some(caps) = count_re().captures(trimmed) {
        let done = caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok());
        let total = caps.get(2).and_then(|m| m.as_str().parse::<u64>().ok());
        if let Some(d) = done {
            progress.done = d;
            progress.build_finished = true;
        }
        if let Some(t) = total {
            progress.total = Some(t);
        }
        return done.is_some();
    }

    false
}

fn apply_event(ev: &LibtestEvent, progress: &mut TestProgress) -> bool {
    match ev.kind.as_str() {
        "suite" => {
            if ev.event.as_deref() == Some("started") {
                if let Some(c) = ev.test_count {
                    // Multiple suites accumulate; add to any existing total.
                    progress.total = Some(progress.total.unwrap_or(0) + c);
                }
                progress.build_finished = true;
            }
            true
        }
        "test" => {
            match ev.event.as_deref() {
                // A test finishing (ok/failed/ignored) advances the count; a
                // "started" event does not (it would double-count).
                Some("ok") | Some("failed") | Some("ignored") => {
                    progress.done += 1;
                    progress.build_finished = true;
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Final pass/fail counts parsed from the stable human summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TestTotals {
    pub run: u64,
    pub passed: u64,
    pub failed: u64,
}

/// Parse nextest's `Summary [ ... ] N tests run: P passed[, F failed]` line.
pub fn parse_nextest_summary(text: &str) -> Option<TestTotals> {
    let re = nextest_summary_re();
    let caps = text.lines().rev().find_map(|l| re.captures(l))?;
    let run = caps.get(1)?.as_str().parse().ok()?;
    let passed = caps.get(2)?.as_str().parse().ok()?;
    let failed = caps
        .get(3)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);
    Some(TestTotals {
        run,
        passed,
        failed,
    })
}

/// Parse the aggregate of `cargo test`'s `test result:` lines.
pub fn parse_cargo_test_summary(text: &str) -> Option<TestTotals> {
    let re = cargo_test_result_re();
    let mut totals = TestTotals::default();
    let mut any = false;
    for caps in text.lines().filter_map(|l| re.captures(l)) {
        let passed: u64 = caps
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let failed: u64 = caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        totals.passed += passed;
        totals.failed += failed;
        any = true;
    }
    if !any {
        return None;
    }
    totals.run = totals.passed + totals.failed;
    Some(totals)
}

// --- Lazily-compiled regexes ------------------------------------------------

fn starting_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Starting\s+(\d+)\s+tests?").expect("valid regex"))
}

fn count_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\(\s*(\d+)\s*/\s*(\d+)\s*\)").expect("valid regex"))
}

fn nextest_summary_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"Summary\s+\[.*?\]\s+(\d+)\s+tests? run:\s+(\d+)\s+passed(?:.*?(\d+)\s+failed)?",
        )
        .expect("valid regex")
    })
}

fn cargo_test_result_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"test result:.*?(\d+)\s+passed;\s+(\d+)\s+failed").expect("valid regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_args_for_workspace_and_crate_and_apponly() {
        assert_eq!(scope_args(&Scope::Workspace, &[]), vec!["--workspace"]);
        assert_eq!(
            scope_args(&Scope::Crate { name: "a".into() }, &[]),
            vec!["-p", "a"]
        );
        let orphans = vec!["orphan1".to_string(), "orphan2".to_string()];
        assert_eq!(
            scope_args(
                &Scope::AppOnly {
                    closure_of: "app".into()
                },
                &orphans
            ),
            vec![
                "--workspace",
                "--exclude",
                "orphan1",
                "--exclude",
                "orphan2"
            ]
        );
    }

    #[test]
    fn cargo_build_args_scope_workspace_no_release() {
        // Validates: Requirement 15.2 -- bare --build builds the active scope.
        assert_eq!(
            cargo_build_args(&Scope::Workspace, &[], &BuildTarget::Scope, false),
            vec!["build", "--workspace"]
        );
    }

    #[test]
    fn cargo_build_args_scope_crate() {
        assert_eq!(
            cargo_build_args(
                &Scope::Crate { name: "a".into() },
                &[],
                &BuildTarget::Scope,
                false
            ),
            vec!["build", "-p", "a"]
        );
    }

    #[test]
    fn cargo_build_args_app_only_with_orphans() {
        let orphans = vec!["o1".to_string(), "o2".to_string()];
        assert_eq!(
            cargo_build_args(
                &Scope::AppOnly {
                    closure_of: "app".into()
                },
                &orphans,
                &BuildTarget::Scope,
                false
            ),
            vec!["build", "--workspace", "--exclude", "o1", "--exclude", "o2"]
        );
    }

    #[test]
    fn cargo_build_args_named_bin() {
        // Validates: Requirement 15.3 -- --build <bin> builds -p <bin>.
        assert_eq!(
            cargo_build_args(
                &Scope::Workspace,
                &[],
                &BuildTarget::Bin("mybin".into()),
                false
            ),
            vec!["build", "-p", "mybin"]
        );
    }

    #[test]
    fn cargo_build_args_release_appends_flag() {
        // Validates: Requirement 15.6 -- --release composes with --build.
        assert_eq!(
            cargo_build_args(&Scope::Workspace, &[], &BuildTarget::Scope, true),
            vec!["build", "--workspace", "--release"]
        );
        assert_eq!(
            cargo_build_args(
                &Scope::Workspace,
                &[],
                &BuildTarget::Bin("mybin".into()),
                true
            ),
            vec!["build", "-p", "mybin", "--release"]
        );
    }

    #[test]
    fn parses_total_from_nextest_list_json() {
        let json = r#"{
            "rust-suites": {
                "crate-a::bin": { "test-count": 3 },
                "crate-b::bin": { "test-count": 5 }
            }
        }"#;
        assert_eq!(parse_total_from_list(json), Some(8));
    }

    #[test]
    fn parses_total_by_counting_testcases_when_count_absent() {
        let json = r#"{
            "rust-suites": {
                "crate-a::bin": { "testcases": { "t1": {}, "t2": {} } }
            }
        }"#;
        assert_eq!(parse_total_from_list(json), Some(2));
    }

    #[test]
    fn ingest_libtest_suite_started_sets_total() {
        let mut p = TestProgress::default();
        let advanced = ingest_line(
            r#"{ "type": "suite", "event": "started", "test_count": 42 }"#,
            &mut p,
        );
        assert!(advanced);
        assert_eq!(p.total, Some(42));
        assert!(p.build_finished);
    }

    #[test]
    fn ingest_libtest_test_ok_advances_done() {
        let mut p = TestProgress::default();
        ingest_line(r#"{ "type": "test", "event": "ok", "name": "x" }"#, &mut p);
        ingest_line(
            r#"{ "type": "test", "event": "failed", "name": "y" }"#,
            &mut p,
        );
        ingest_line(
            r#"{ "type": "test", "event": "started", "name": "z" }"#,
            &mut p,
        );
        assert_eq!(p.done, 2, "only ok/failed advance; started does not");
    }

    #[test]
    fn ingest_human_fallback_count_marker() {
        let mut p = TestProgress::default();
        let advanced = ingest_line("    Running (  12/100) some::test", &mut p);
        assert!(advanced);
        assert_eq!(p.done, 12);
        assert_eq!(p.total, Some(100));
    }

    #[test]
    fn ingest_human_fallback_starting_marker() {
        let mut p = TestProgress::default();
        let advanced = ingest_line("    Starting 250 tests across 10 binaries", &mut p);
        assert!(advanced);
        assert_eq!(p.total, Some(250));
        assert!(p.build_finished);
    }

    #[test]
    fn parses_nextest_summary_with_failures() {
        let text = "   Summary [  12.345s] 5000 tests run: 4998 passed, 2 failed";
        let t = parse_nextest_summary(text).expect("summary");
        assert_eq!(
            t,
            TestTotals {
                run: 5000,
                passed: 4998,
                failed: 2
            }
        );
    }

    #[test]
    fn parses_nextest_summary_all_passing() {
        let text = "   Summary [  1.00s] 10 tests run: 10 passed, 0 skipped";
        let t = parse_nextest_summary(text).expect("summary");
        assert_eq!(t.run, 10);
        assert_eq!(t.passed, 10);
        assert_eq!(t.failed, 0);
    }

    #[test]
    fn parses_cargo_test_result_aggregate() {
        let text = "\
test result: ok. 10 passed; 0 failed; 1 ignored
test result: FAILED. 3 passed; 2 failed; 0 ignored";
        let t = parse_cargo_test_summary(text).expect("totals");
        assert_eq!(
            t,
            TestTotals {
                run: 15,
                passed: 13,
                failed: 2
            }
        );
    }

    #[test]
    fn cargo_test_summary_none_when_no_results() {
        assert_eq!(parse_cargo_test_summary("Compiling foo"), None);
    }
}
