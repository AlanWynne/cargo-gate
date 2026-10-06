//! The progress display: a print-on-completion model.
//!
//! The gate shows NOTHING for a phase until that phase finishes. When a phase
//! completes, exactly one line is printed for it (`<name> [done] <summary>`),
//! in run order. There is no persistent progress board and no placeholder /
//! empty / "pending" row: a half-drawn or lingering row is exactly the defect
//! this model avoids. The run reads top to bottom as:
//!
//! ```text
//! started ...   estimated completion ...
//! cargo-gate: mode=... runner=... scope=...
//! fmt       [done] 0:00:00  ok
//! clippy    [done] 0:00:00  clean
//! build     [done] 0:00:00
//! test      [done] 73/73 passed  0:00:10
//! app-build [done] 0:00:01  ok        (only with --build)
//! completed ...   actual ...
//! cargo-gate: CLEAN -- ...
//! ```
//!
//! The `done/total` test count is still tracked internally (so the test phase
//! summary can report it), but no live animated bar is drawn: the user prefers
//! clean print-on-completion over an animated row that may not clear cleanly on
//! every terminal. All lines route through `println`, which respects quiet mode
//! (non-TTY / CI) by using a plain `println!`.

use std::cell::Cell;
use std::time::Duration;

use crate::phase::Step;

/// Prints one finished line per phase, in order, as each phase completes.
pub struct Display {
    /// Internal test done/total tracking (not drawn live). `None` length means
    /// the total is not yet known.
    test_total: Cell<Option<u64>>,
    test_done: Cell<u64>,
    quiet: bool,
}

impl Display {
    /// Create the display. `quiet` (no TTY / CI) only affects how `println`
    /// emits; the print-on-completion model is identical in both modes.
    pub fn new(quiet: bool) -> Self {
        Display {
            test_total: Cell::new(None),
            test_done: Cell::new(0),
            quiet,
        }
    }

    /// A phase has started. Nothing is drawn: no row appears until the phase
    /// finishes (print-on-completion). Kept for call-site symmetry.
    pub fn start_spinner(&self, _step: Step, _eta: Option<Duration>) {}

    /// A captured line advanced the phase. No live row is drawn, so this is a
    /// no-op; progress is reflected only in the final one-line summary.
    pub fn tick(&self, _step: Step) {}

    /// The test phase has begun counting. Records the known total; draws
    /// nothing.
    pub fn start_test(&self, total: u64) {
        self.test_total.set(Some(total));
    }

    /// Update the known test total. Draws nothing.
    pub fn set_test_total(&self, total: u64) {
        self.test_total.set(Some(total));
    }

    /// Update the running test done count. Draws nothing; reflected in the
    /// final summary.
    pub fn set_test_done(&self, done: u64) {
        self.test_done.set(done);
    }

    /// Print the single finished line for a completed phase, in run order.
    pub fn finish_phase(&self, step: Step, summary: &str) {
        self.println(&format!("{:<9} [done] {}", step.id(), summary));
    }

    /// No persistent bars exist, so there is nothing to flush before the
    /// trailing footer and verdict. Kept for call-site symmetry.
    pub fn finish_all(&self) {}

    /// Print a line. Plain `println!` in all modes (there is no live bar area to
    /// print above). `quiet` is retained for API symmetry and future use.
    pub fn println(&self, line: &str) {
        let _ = self.quiet;
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The display is now a print-on-completion model with no live bars; these
    // assert the lifecycle calls do not panic and internal test tracking holds.
    #[test]
    fn display_lifecycle_does_not_panic() {
        let d = Display::new(true);
        d.start_spinner(Step::Fmt, Some(Duration::from_secs(2)));
        d.tick(Step::Fmt);
        d.finish_phase(Step::Fmt, "0.4s");
        d.start_spinner(Step::Test, None);
        d.start_test(100);
        d.set_test_done(50);
        d.finish_phase(Step::Test, "100/100 12.0s");
        // The optional app-build phase drives the same lifecycle.
        d.start_spinner(Step::AppBuild, Some(Duration::from_secs(20)));
        d.tick(Step::AppBuild);
        d.finish_phase(Step::AppBuild, "0:00:18  ok");
        d.finish_all();
        d.println("hello");
    }

    // Internal test done/total tracking is monotonic and preserved across a
    // total change (no reset to 0), so the final summary count stays correct.
    #[test]
    fn test_total_and_done_tracking_is_preserved() {
        let d = Display::new(true);
        d.start_test(100);
        d.set_test_done(50);
        assert_eq!(d.test_done.get(), 50);
        d.set_test_total(120);
        assert_eq!(d.test_done.get(), 50, "a total change must not reset done");
        assert_eq!(d.test_total.get(), Some(120));
        d.set_test_done(60);
        assert_eq!(d.test_done.get(), 60);
    }
}
