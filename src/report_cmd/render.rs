//! Pure Markdown rendering of the wiring report. `render_report` builds the
//! whole document as an ASCII string from the computed `Wiring`, the optional
//! gate `RunSummary`, and the root provenance, so tests assert on substrings
//! and section ordering without any IO.

use std::fmt::Write as _;

use super::roots::RootProvenance;
use super::summary::RunSummary;
use super::wiring::Wiring;

// === Fixed text =====================================================

/// Section heading for the gate run summary (Req 7.1).
const H_SUMMARY: &str = "## Gate run summary";
/// Section heading for the roots list (Req 7.2).
const H_ROOTS: &str = "## Roots";
/// Section heading for the wired-in crates (Req 7.3).
const H_WIRED: &str = "## Wired-in crates";
/// Section heading for the orphan crates (Req 7.4).
const H_ORPHANS: &str = "## Orphan crates";
/// Section heading for the per-root dependency trees (Req 7.5).
const H_TREE: &str = "## Dependency tree";

/// The honesty label (Req 6.1, 7.6). Distinguishes dependency-graph wiring from
/// functional runtime registration.
const HONESTY_LABEL: &str = concat!(
    "> NOTE: This report measures DEPENDENCY-GRAPH wiring -- a crate appears in ",
    "a root's cargo dependency tree. It does NOT measure FUNCTIONAL ",
    "registration (whether the crate is actually dispatched or registered at ",
    "runtime). A declared dependency can still be functionally unreachable."
);

// === Rendering ======================================================

/// Render the full Markdown wiring report.
pub fn render_report(
    wiring: &Wiring,
    summary: &Option<RunSummary>,
    provenance: &RootProvenance,
) -> String {
    let mut out = String::new();
    out.push_str("# Workspace wiring report\n\n");
    render_summary(&mut out, summary);
    render_roots(&mut out, wiring, provenance);
    render_wired_in(&mut out, wiring);
    render_orphans(&mut out, wiring);
    render_tree(&mut out, wiring);
    out
}

fn render_summary(out: &mut String, summary: &Option<RunSummary>) {
    let _ = writeln!(out, "{H_SUMMARY}\n");
    match summary {
        None => {
            let _ = writeln!(
                out,
                "No prior gate run recorded (no history found under the log dir).\n"
            );
        }
        Some(s) => {
            let _ = writeln!(out, "- Verdict: {}", s.verdict.as_str());
            let _ = writeln!(
                out,
                "- Run: {} (mode {}, scope {})",
                s.run_id, s.mode, s.scope
            );
            for step in &s.steps {
                let _ = writeln!(
                    out,
                    "- {}: {} ({})",
                    step.step,
                    step.status,
                    step.duration_hms()
                );
            }
            match &s.test_counts {
                Some(tc) => {
                    let _ = writeln!(
                        out,
                        "- Tests: {} total, {} passed, {} failed",
                        tc.total, tc.passed, tc.failed
                    );
                }
                None => {
                    let _ = writeln!(out, "- Tests: counts unavailable");
                }
            }
            out.push('\n');
        }
    }
}

fn render_roots(out: &mut String, wiring: &Wiring, provenance: &RootProvenance) {
    let _ = writeln!(out, "{H_ROOTS}\n");
    let label = match provenance {
        RootProvenance::AutoDetected => "auto-detected",
        RootProvenance::UserSpecified => "user-specified",
    };
    let _ = writeln!(out, "Roots were {label}.\n");
    if wiring.roots.names.is_empty() {
        let _ = writeln!(out, "No roots (empty workspace).\n");
        return;
    }
    for root in &wiring.roots.names {
        let degraded = if wiring.degraded_roots.contains(root) {
            " (closure degraded: cargo tree unavailable, root only)"
        } else {
            ""
        };
        let _ = writeln!(out, "- {root}{degraded}");
    }
    out.push('\n');
}

fn render_wired_in(out: &mut String, wiring: &Wiring) {
    let _ = writeln!(out, "{H_WIRED}\n");
    let _ = writeln!(out, "{HONESTY_LABEL}\n");
    if wiring.wired_in.is_empty() {
        let _ = writeln!(out, "No wired-in crates.\n");
        return;
    }
    for (crate_name, roots) in &wiring.wired_in {
        let attribution = roots
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "- {crate_name} (via {attribution})");
    }
    out.push('\n');
}

fn render_orphans(out: &mut String, wiring: &Wiring) {
    let _ = writeln!(out, "{H_ORPHANS}\n");
    if wiring.orphans.is_empty() {
        let _ = writeln!(
            out,
            "Every workspace member is wired in (no orphan crates).\n"
        );
        return;
    }
    for orphan in &wiring.orphans {
        let _ = writeln!(out, "- {orphan}");
    }
    out.push('\n');
}

fn render_tree(out: &mut String, wiring: &Wiring) {
    let _ = writeln!(out, "{H_TREE}\n");
    if wiring.closures.is_empty() {
        let _ = writeln!(out, "No roots to show.\n");
        return;
    }
    for (root, closure) in &wiring.closures {
        let _ = writeln!(out, "- {root}");
        for member in closure {
            if member == root {
                continue;
            }
            let _ = writeln!(out, "  - {member}");
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase::Verdict;
    use crate::report_cmd::roots::Roots;
    use crate::report_cmd::summary::{RunSummary, StepSummary, TestCounts};
    use crate::report_cmd::wiring::Wiring;
    use crate::workspace::CrateName;
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    fn cn(s: &str) -> CrateName {
        CrateName(s.to_string())
    }

    fn set(names: &[&str]) -> BTreeSet<CrateName> {
        names.iter().map(|n| cn(n)).collect()
    }

    fn sample_wiring(provenance: RootProvenance) -> Wiring {
        let mut wired_in: BTreeMap<CrateName, BTreeSet<CrateName>> = BTreeMap::new();
        wired_in.insert(cn("app"), set(&["app"]));
        wired_in.insert(cn("lib-a"), set(&["app", "ffmdx"]));
        wired_in.insert(cn("lib-b"), set(&["ffmdx"]));
        wired_in.insert(cn("ffmdx"), set(&["ffmdx"]));

        let mut closures: BTreeMap<CrateName, BTreeSet<CrateName>> = BTreeMap::new();
        closures.insert(cn("app"), set(&["app", "lib-a"]));
        closures.insert(cn("ffmdx"), set(&["ffmdx", "lib-a", "lib-b"]));

        Wiring {
            roots: Roots {
                names: set(&["app", "ffmdx"]),
                provenance,
            },
            wired_in,
            orphans: set(&["orphan-x"]),
            closures,
            degraded_roots: BTreeSet::new(),
        }
    }

    fn sample_summary() -> RunSummary {
        RunSummary {
            run_id: "r2".to_string(),
            mode: "FULL".to_string(),
            scope: "workspace".to_string(),
            verdict: Verdict::Clean,
            steps: vec![StepSummary {
                step: "test".to_string(),
                duration: Duration::from_secs(120),
                status: "ok".to_string(),
            }],
            test_counts: Some(TestCounts {
                total: 200,
                passed: 200,
                failed: 0,
            }),
        }
    }

    // Validates: Requirement 6.1, 7.6 -- the honesty label text is present.
    #[test]
    fn report_contains_honesty_label() {
        let w = sample_wiring(RootProvenance::AutoDetected);
        let md = render_report(&w, &Some(sample_summary()), &RootProvenance::AutoDetected);
        assert!(md.contains("DEPENDENCY-GRAPH wiring"));
        assert!(md.contains("FUNCTIONAL"));
        assert!(md.contains("functionally unreachable"));
    }

    // Validates: Requirement 7.1-7.5 -- the five section headings appear in order.
    #[test]
    fn five_section_headings_appear_in_order() {
        let w = sample_wiring(RootProvenance::AutoDetected);
        let md = render_report(&w, &Some(sample_summary()), &RootProvenance::AutoDetected);
        let i_summary = md.find(H_SUMMARY).expect("summary heading");
        let i_roots = md.find(H_ROOTS).expect("roots heading");
        let i_wired = md.find(H_WIRED).expect("wired heading");
        let i_orphans = md.find(H_ORPHANS).expect("orphans heading");
        let i_tree = md.find(H_TREE).expect("tree heading");
        assert!(i_summary < i_roots);
        assert!(i_roots < i_wired);
        assert!(i_wired < i_orphans);
        assert!(i_orphans < i_tree);
    }

    // Validates: Requirement 4.3 -- with no orphans the section states every
    // member is wired in.
    #[test]
    fn no_orphans_states_every_member_wired_in() {
        let mut w = sample_wiring(RootProvenance::AutoDetected);
        w.orphans = BTreeSet::new();
        let md = render_report(&w, &Some(sample_summary()), &RootProvenance::AutoDetected);
        assert!(md.contains("Every workspace member is wired in"));
    }

    // Validates: Requirement 2.6 / AC-15 -- roots section states auto-detected
    // vs user-specified.
    #[test]
    fn roots_section_states_provenance() {
        let w_auto = sample_wiring(RootProvenance::AutoDetected);
        let md_auto = render_report(&w_auto, &None, &RootProvenance::AutoDetected);
        assert!(md_auto.contains("Roots were auto-detected."));

        let w_user = sample_wiring(RootProvenance::UserSpecified);
        let md_user = render_report(&w_user, &None, &RootProvenance::UserSpecified);
        assert!(md_user.contains("Roots were user-specified."));
    }

    // Validates: Requirement 8.3 / AC-10 -- the no-history note appears when the
    // summary is None, and the wiring sections are still present.
    #[test]
    fn no_history_note_appears_when_summary_is_none() {
        let w = sample_wiring(RootProvenance::AutoDetected);
        let md = render_report(&w, &None, &RootProvenance::AutoDetected);
        assert!(md.contains("No prior gate run recorded"));
        assert!(md.contains(H_ROOTS));
        assert!(md.contains(H_WIRED));
        assert!(md.contains(H_ORPHANS));
        assert!(md.contains(H_TREE));
    }

    // Validates: Requirement 10.1 -- rendered output is ASCII only.
    #[test]
    fn rendered_output_is_ascii_only() {
        let w = sample_wiring(RootProvenance::AutoDetected);
        let md = render_report(&w, &Some(sample_summary()), &RootProvenance::AutoDetected);
        assert!(md.is_ascii(), "report must be ASCII only");
    }

    // Validates: Requirement 5.1, 5.2 -- attribution is shown in the wired-in
    // listing (lib-a via both roots, lib-b via ffmdx only).
    #[test]
    fn wired_in_listing_shows_attribution() {
        let w = sample_wiring(RootProvenance::AutoDetected);
        let md = render_report(&w, &None, &RootProvenance::AutoDetected);
        assert!(md.contains("lib-a (via app, ffmdx)"));
        assert!(md.contains("lib-b (via ffmdx)"));
    }
}
