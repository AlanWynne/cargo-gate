# cargo-gate -- Design

Architecture and design decisions. Pairs with `requirements.md` (the WHAT) by
describing the HOW. Read this before changing module boundaries or data flow.

## 1. Overview

cargo-gate is a single binary that orchestrates child `cargo` processes, parses
their output into a structured per-step model, drives a live multi-bar display,
and emits durable artifacts plus a verdict. The design goal that shapes
everything: **replace fragile text-scraping with structured output** wherever
cargo/nextest offer it, and keep a regex fallback only where the structured
format is experimental.

```
main.rs
  -> config::Config::from_args          (parse argv, strip `gate` token)
  -> gate::run(&config)
       -> workspace::workspace_root / derive_orphans
       -> detect runner (nextest?)
       -> display::Display::new
       -> for each phase:
            runner::run_with_capture  --(stderr lines)-->  phase parser + display
       -> optional app-build phase (run_app_build_phase) when --build is set
       -> fold StepResults -> Verdict
       -> report::write_* + history::append_rows + append_overall_row
       -> print start/ETA header, phase [done] lines, completion/variance footer,
          and the verdict; return exit code
```

## 2. Module boundaries

| Module | Responsibility | Key types / fns |
|--------|----------------|-----------------|
| `config` | CLI surface and resolved run config | `Cli`, `Config`, `Mode`, `Scope` |
| `phase` | Shared vocabulary all layers consume | `Step`, `Status`, `StepResult`, `Verdict` |
| `runner` | Child-process execution, capture, watchdog | `run_with_capture`, `RunOutput` |
| `workspace` | Workspace root + app-only orphan derivation | `workspace_root`, `derive_orphans` |
| `phases::fmt_clippy` | fmt and clippy phases + diagnostic counting | `run_fmt`, `run_clippy`, `count_diagnostics` |
| `phases::nextest` | test-runner args, total, live events, totals | `Runner`, `ingest_line`, `parse_*` |
| `display` | print-on-completion phase lines (no live bar) | `Display`, `finish_phase` |
| `history` | long-format CSV + per-step and whole-run ETA | `append_rows`, `append_overall_row`, `eta_for_step`, `overall_eta_total`, `overall_eta` |
| `report` | combined/review/timing artifacts + filtering + variance | `write_combined_report`, `write_review_log`, `write_timing_log`, `format_hms`, `format_variance` |
| `gate` | orchestration: wires all of the above | `run`, `run_test_phase`, `run_app_build_phase` |

Dependency direction: `gate` depends on everything; `phases` depend on
`config`/`phase`/`runner`; `phase` and `config` depend on nothing internal. No
cycles.

## 3. Key decisions

### 3.1 Structured output over scraping
- **clippy:** run with `--message-format=json`; parse `compiler-message` lines
  with a tolerant serde struct and count by `level` (`error`/`warning`). This is
  stable cargo JSON (see rust-lang cargo "external tools" docs) and gives exact
  counts, replacing the PowerShell original's `^error`/`^warning` regexes.
- **test total:** `cargo nextest list --message-format json` is STABLE
  machine-readable output; we sum `test-count` across `rust-suites`. Knowing N up
  front is what lets the test bar be a true `done/total` bar rather than a spinner.
- **live test events:** nextest's libtest-json (`--message-format libtest-json`,
  gated behind `NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1`) is EXPERIMENTAL and
  documented as not full-fidelity. We parse it when present but NEVER depend on it
  alone: `ingest_line` tries the JSON event first, then falls back to the human
  `(n/N)` and `Starting N tests` markers. The authoritative pass/fail counts come
  from the stable human `Summary [...]` line, not the experimental stream.

Rationale: the experimental format can change between nextest versions; the
layered strategy means a format bump degrades the bar's smoothness at worst, never
correctness.

### 3.2 Build vs test split
nextest/cargo compile the test binaries, then run them, in one invocation. We run
it ONCE and split the timing: the first "tests starting" signal
(`suite started` event, or the `Starting N tests` / first `(n/N)` line) marks the
end of the build phase and the start of the test phase. `run_test_phase` tracks
`build_done_at` and derives `build_elapsed` and `run_elapsed` from one wall clock.
This gives the user the "build" vs "run" distinction without running cargo twice.

### 3.3 Child execution and the watchdog (`runner.rs`)
`run_with_capture` spawns the child with piped stdout/stderr. stdout is pumped to
its raw log on a dedicated thread. stderr is pumped to its raw log AND each line
forwarded over an mpsc channel to the main loop, which (a) invokes the live
callback (so a phase can parse progress) and (b) polls `try_wait` and the clock.
If the elapsed time exceeds the timeout, the child is killed and `timed_out` is
set. This keeps parsing, logging, and the watchdog on independent paths so none
blocks another. `kill_tree` currently kills the direct child (sufficient for
cargo in practice; a full process-tree kill is a noted future hardening,
especially for Unix).

After the child exits, the pump threads are joined with `join_with_grace`
(a bounded 5s join), NOT a bare `join()`. A failing/interrupted test run can
leave a grandchild test-binary process holding the stdout pipe's write end open,
so `BufReader::lines()` never sees EOF and the pump thread never returns; a bare
`join()` would then hang the whole gate forever. Since the raw logs are
best-effort and the exit status is already captured, a pump thread that overruns
the grace period is detached rather than waited on. (Fixed after a real hang on a
failing FFWB run; regression test `failing_child_with_output_returns_promptly`.)

### 3.4 The per-step model (`phase.rs`)
Everything downstream speaks `StepResult { step, status, duration, warnings,
errors, items_* , review_lines }`. `Verdict::from_results` folds: any TimedOut ->
TIMEOUT; else any Issues -> ISSUES; else CLEAN. `Status::Skipped` is used when the
build fails and the tests never run, so a failed build does not masquerade as a
passing (or absent) test phase. `Step` has five variants: `Fmt`, `Clippy`,
`Build` (compiling the test binaries), `Test`, and `AppBuild` -- the optional
application build from `--build`, kept distinct from `Build` so its very
different duration trends independently in history and ETAs.

### 3.4a Optional application build (`--build`, Requirement 15)
By default the gate is verification-only and produces NO runnable application
binary (clippy compiles for analysis; the test phase links test binaries). The
opt-in `--build` flag adds an `AppBuild` phase AFTER the test phase: when the gate
so far is clean it runs `cargo build` for the active scope (or `-p <bin>` for
`--build <bin>`, `--release` composable), as a real timed phase that lands in the
display, report, timing log, and a history row. If the gate is already failing it
is skipped (`Status::Skipped`, "gate not clean"); a build failure folds to ISSUES.
Off by default keeps the default a pure, side-effect-free CI gate.

### 3.5 Long-format history (`history.rs`)
One row PER STEP per run. This was a deliberate choice over a wide one-row-per-run
format: it lets fmt / clippy / build / test each be trended and predicted
independently ("clippy got 40% slower"), and still rolls up to a per-run view via
`run_id`. `eta_for_step` filters rows to the same `(step, mode, scope)`, takes the
most recent `window` (10), and returns the median -- robust to outliers and to a
quick `--fast` run not polluting a full-run ETA.

Two kinds of ETA are derived (Requirement 14):
- **Per-phase ETA** (`eta_for_step`): drives the per-phase display and the rough
  fallback estimate. `overall_eta` sums the per-phase medians.
- **Whole-run ETA** (`overall_eta_total`): the HEADLINE estimate. Each run writes
  one extra row with `step = "overall"` and `duration_secs` = total end-to-end
  wall clock (`append_overall_row`), and the headline ETA is the median of past
  "overall" rows. This is deliberately consistent with the completion footer's
  "actual" (both are end-to-end wall clock), so the reported variance reflects
  real drift -- NOT the structural gap between a sum-of-phases estimate and wall
  clock. The wall clock includes the untimed `nextest list` pre-pass, workspace
  discovery, orphan derivation, and inter-phase overhead, which the per-phase sum
  misses (that mismatch caused +80..+217% variance on FFWB before this fix). The
  caller uses `overall_eta_total` when present and falls back to `overall_eta`
  only until the first "overall" row exists. "overall" is just another step label
  in the same CSV schema -- no new columns, and it never matches a per-phase ETA
  filter.

### 3.6 Report filtering (`report.rs`)
The combined report keeps signal and drops noise, matching the ffwb-gate intent:
`is_noise` drops blank lines, `... ok` per-test lines, and nextest `PASS [` lines;
everything else (section headers, `Compiling`, warnings, errors, group summaries)
is kept. The review log is the concatenation of every step's `review_lines`;
empty == clean is the contract CI relies on.

### 3.7 Display (`display.rs`) -- print-on-completion
The display is a PRINT-ON-COMPLETION model, NOT a live animated board (indicatif
was removed). Nothing is shown for a phase until it finishes; on completion
exactly one line is printed (`<name> [done] <summary>`), in run order. The run
reads top to bottom as: start/ETA header -> mode/runner/scope banner -> one
`[done]` line per phase (fmt, clippy, build, test, and app-build when `--build`)
-> completion/variance footer -> final verdict. The test done/total count is
still tracked internally so the test line can report `N/N passed`, but no live
bar is drawn.

Rationale: an earlier live `MultiProgress` board left half-drawn / lingering
"pending" rows and raced the trailing summary prints on this environment's
terminal. Print-on-completion is deterministic, has no placeholder rows, and
reads identically whether stdout is a TTY or redirected (quiet mode uses the same
ordering). All lines route through `Display::println`.

### 3.8 Scope and app-only (`workspace.rs`)
`derive_orphans` runs `cargo metadata --no-deps` for the member list and
`cargo tree -p <crate> --edges normal --prefix none` for the dependency closure,
then returns members minus closure. Deriving at runtime means a crate newly wired
into the app automatically re-enters the gate -- the exclude list never drifts
stale. On any failure it returns empty and the caller falls back to full
`--workspace` (never a wrong subset).

## 4. Data flow for one test run (detailed)

1. `run_test_phase` optionally calls `list_total` (nextest list) -> `Option<u64>`.
2. Starts the build spinner; builds args + env (libtest-json for nextest).
3. `run_with_capture` streams lines; the callback calls
   `nextest::ingest_line(line, &mut progress)`:
   - on first `build_finished`: finish build bar, start test bar, set length;
   - on advance: update `done`, refresh bar; else tick the build spinner.
4. On exit: compute `build_elapsed` (to the transition) and `run_elapsed`
   (remainder); read raw logs; parse the summary for authoritative totals.
5. `classify_test` decides Ok/Issues/TimedOut and collects FAIL lines; a failed
   build yields a Skipped test result instead.
6. Returns `(build_result, test_result)`.

## 5. Error handling

- Library-style modules return `anyhow::Result` with `.context(...)`.
- A child that fails to spawn is a hard error (bubbles to `main`, exit 2).
- A phase that runs but exits non-zero is NOT a hard error -- it is a `Status`
  the verdict folds. The gate always completes and writes artifacts unless a
  spawn/IO failure makes that impossible.

## 6. Testing strategy

- Pure functions (parsers, counting, filtering, ETA medians incl. whole-run,
  variance formatting, verdict folding, arg building incl. `--build`, token
  stripping) have focused unit tests -- the bulk of the 75.
- `runner` has two OS-level tests (capture success; watchdog kill) gated to
  Windows for now; add Unix equivalents.
- `display` has lifecycle tests with a hidden draw target (construction + state
  transitions do not panic; rendering itself is not asserted).
- NEXT (see STATUS.md section 5): an integration test that runs the built binary
  against a fixture workspace and asserts artifacts + verdict + exit code.

## 7. Deliberate non-goals (first cut)

- No parallel phase execution (fmt and clippy could overlap; kept sequential for a
  clean linear display and simpler verdict reasoning).
- No `--fix` / auto-format mode yet. (`--build` leaves a runnable binary but does
  not modify source.)
- No config file; all configuration is CLI flags.
- No JUnit/other machine export of the gate's own result (the history CSV is the
  structured output).
