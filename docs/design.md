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

## 8. The `report` subcommand -- workspace wiring report (new feature)

This section EXTENDS the design for the new `cargo gate report` feature. It adds
modules and a CLI subcommand; it does NOT change any decision in sections 1-7.
The one deliberate change to a prior assumption is called out in 8.1.

### 8.1 CLI shape: a real subcommand (change to the prior CLI model)

PRIOR DECISION (section 2, `config`): `Cli` was a FLAT clap-derive struct of
flags with no subcommands, and `main` always calls `gate::run`. The report
feature requires a mode that does NOT run the gate, so this is a deliberate,
owner-approved CHANGE to the CLI model: `Cli` gains an OPTIONAL subcommand.

```
Cli {
    #[command(subcommand)] command: Option<Command>,  // NEW
    // ... all existing flags unchanged ...
}

enum Command {
    Report(ReportArgs),
}
```

- WHEN `command` is `None` (no subcommand) the existing gate path runs with the
  existing flags, byte-for-byte as before (Req 1.2). The flags stay on the top
  -level `Cli` so `cargo gate --fast` etc. are untouched; only the NEW optional
  subcommand is added. This keeps the change additive (Req 11.1, NFR-2).
- WHEN `command` is `Some(Command::Report(args))` the new report path runs and
  the gate is not invoked.

`ReportArgs` (new, report-only flags):

```
ReportArgs {
    /// Repeatable root override; an explicit list replaces auto-detection.
    #[arg(long = "root", value_name = "CRATE")] roots: Vec<String>,
    /// Output path override; default <log-dir>/gate.wiring-report.md.
    #[arg(long = "report-out", value_name = "FILE")] out: Option<PathBuf>,
    /// Log dir (shared meaning with the gate; default .gate).
    #[arg(long, value_name = "DIR")] log_dir: Option<PathBuf>,
}
```

Chosen flag names, recorded as the new public contract (Req 9, 11): subcommand
`report`; `--root <CRATE>` (repeatable); `--report-out <FILE>`; `--log-dir <DIR>`
(same meaning as the gate's). Semver: a MINOR bump (additive subcommand + flags).
`Cargo.toml` `version` moves `0.1.0 -> 0.2.0` as part of this feature.

`strip_subcommand_token` (drops the leading `gate` token for the cargo-subcommand
invocation) is UNCHANGED and runs before clap parses; the new `report` token is a
normal clap subcommand after that strip.

### 8.2 Reuse of `workspace.rs` (NFR-5) -- extend, do not re-implement

The report reuses the existing `workspace` module and extends it minimally:

- `workspace_root(cargo, cwd)` -- reused as-is to resolve the output dir base.
- `member_names(cargo, cwd)` -- reused as-is for the full member list.
- `dependency_closure(cargo, cwd, root)` -- currently PRIVATE
  (`fn dependency_closure`). CHANGE: make it `pub(crate)` so the report module
  can compute a per-root closure. No behaviour change; same `cargo tree -p <root>
  --edges normal --prefix none` parsing, same "root always included" guarantee,
  same `Option<HashSet<String>>` return (None -> degraded closure of just the
  root, Req 3.3).
- `derive_orphans(cargo, cwd, closure_of)` -- the SINGLE-root orphan helper is
  retained unchanged (the gate's `--app-only` still uses it). The report needs
  the MULTI-root union, so it does NOT call `derive_orphans`; instead it computes
  the union of per-root closures and subtracts from members (8.4). The two share
  the same primitives (`member_names`, `dependency_closure`) so there is one
  metadata/tree implementation, satisfying NFR-5.

To support auto-detection (Req 2.2, 2.3) the module needs two facts the current
`Metadata`/`Package` structs do not capture: each member's TARGET KINDS (to know
which members produce a binary) and the inter-member dependency edges (to find
top-of-tree libraries). This is additive to `workspace.rs`:

- Extend the `Package` serde struct with `id`, `targets: Vec<Target>` (each
  `Target { kind: Vec<String> }`) and `dependencies: Vec<Dep>` (each
  `Dep { name: String }`), all parsed from the SAME `cargo metadata --no-deps`
  call already made (no extra process). A new `pub(crate) fn workspace_facts(
  cargo, cwd) -> Result<WorkspaceFacts>` returns a typed snapshot:
  `WorkspaceFacts { members: Vec<MemberFact> }`, `MemberFact { name: CrateName,
  produces_bin: bool, depends_on: Vec<CrateName> }`. Auto-detection reads this
  snapshot; it is pure to test once the snapshot is built.

Keeping the new metadata fields on the existing struct and behind one
`cargo metadata` call preserves the "one metadata implementation" property.

### 8.3 New module layout (400-line rule, split by concern)

The report feature lives in a new `report_cmd` module split by concern so no file
exceeds 400 non-test lines (rust-standards). `report.rs` (the gate's artifact
writer) is NOT touched by this feature beyond reuse of `format_hms`.

```
src/report_cmd/
  mod.rs        -- re-exports; `run_report(&ReportConfig) -> Result<i32>` entry
  roots.rs      -- root auto-detection + override resolution (pure over WorkspaceFacts)
  wiring.rs     -- union-closure, orphan derivation, attribution (pure data model)
  summary.rs    -- read latest run from gate.history.csv into a RunSummary
  render.rs     -- pure Markdown rendering: fn render_report(&Wiring, &Option<RunSummary>, &RootProvenance) -> String
```

- `roots.rs`, `wiring.rs`, and `render.rs` are PURE (no process spawning, no IO):
  they take the `WorkspaceFacts` snapshot and the per-root closures as inputs, so
  every acceptance criterion in Req 2-7 is unit-testable with in-memory fixtures
  and string assertions (no cargo invocation in tests).
- `mod.rs` and `summary.rs` do the IO: call `workspace::workspace_facts` /
  `workspace::dependency_closure`, read the history CSV, and write the Markdown.
- `main.rs` gains `mod report_cmd;` and dispatches: `Some(Command::Report) ->
  report_cmd::run_report(...)` mapping the returned code to `ExitCode`
  (0 success, 2 fatal/usage), exactly mirroring how `gate::run` is mapped today.

### 8.4 Data model (newtypes over bare primitives, NFR-6)

```
struct CrateName(String);                 // newtype; Ord for deterministic sort

enum RootProvenance { AutoDetected, UserSpecified }   // Req 2.6 / AC-15

struct Roots { names: BTreeSet<CrateName>, provenance: RootProvenance }

struct Wiring {
    roots: Roots,
    // wired-in crate -> the roots whose closure contains it (attribution, Req 5)
    wired_in: BTreeMap<CrateName, BTreeSet<CrateName>>,
    orphans: BTreeSet<CrateName>,          // Req 4
    // per-root closure, retained for the tree section (Req 7.5)
    closures: BTreeMap<CrateName, BTreeSet<CrateName>>,
    degraded_roots: BTreeSet<CrateName>,   // closures that fell back to {root} (Req 3.3)
}
```

`BTreeSet`/`BTreeMap` give deterministic, sorted iteration for free (Req 4.4,
AC-7) without an explicit sort step. Public types derive `Debug`.

Algorithm (all pure over `WorkspaceFacts` + per-root closures):

- Auto-detect roots (`roots.rs`): `produces_bin` members UNION members that
  appear in no other member's `depends_on` (top-of-tree libs). If `--root` is
  non-empty, that list REPLACES auto-detection; validate each against
  `facts.members` and return an error naming any unknown crate (Req 2.5, AC-3).
  Empty-member workspace -> empty roots with an "empty workspace" note (Req 2.7).
- Union closure + attribution (`wiring.rs`): for each root `r`, intersect its
  `dependency_closure` with the member set; record `r` into `wired_in[c]` for
  every `c` in that closure. `wired_in.keys()` is the union (Req 3.2);
  `orphans = members - wired_in.keys() - roots` (Req 4.1, 4.2).

### 8.5 Gate run summary sourcing from history (Req 8)

`summary.rs` reads `gate.history.csv` (the existing long-format file) WITHOUT
writing it. It reuses the CSV's known column order (documented in `history.rs`
`HEADER`). It parses rows, groups by `run_id`, picks the LAST run_id in file
order (file order is chronological, same assumption `history::read_rows` already
relies on), and folds that run's per-step rows into a `RunSummary { verdict,
steps: Vec<(step, duration, status)>, test_counts: Option<TestCounts> }`. The
verdict is re-derived from the run's statuses using the SAME fold rule as
`Verdict::from_results` (timeout dominates, then issues, else clean) so the report
agrees with the gate. WHEN the file is absent or has no data rows, `summary.rs`
returns `None` and `render.rs` emits the "no prior gate run recorded" note
(Req 8.3, AC-10). A new lightweight row reader is added in `summary.rs` rather
than exposing `history::read_rows`, because the summary needs the status/counts
columns that `history::Row` currently discards; this keeps `history.rs`'s ETA
reader untouched. `format_hms` from `report.rs` is reused for the timings.

### 8.6 Markdown rendering (`render.rs`, pure)

`render_report(...) -> String` builds the whole document as a `String` from the
`Wiring`, the `Option<RunSummary>`, and the `RootProvenance`. It is pure and
deterministic, so tests assert on substrings and section ordering (AC-8, AC-9,
AC-14, AC-15). Section order matches Req 7.1-7.5. The honesty label (Req 6, 7.6)
is emitted as a fixed ASCII block immediately under the "Wired-in crates"
heading. The dependency-tree section (Req 7.5) renders each root's closure as an
indented list derived from the retained `closures` data (a simple
root-then-sorted-members block; it reuses the already-captured closure rather
than shelling out to `cargo tree` a second time). All output is ASCII (Req 10.1).

### 8.7 Error handling (new code)

- A new `ReportError` enum via `thiserror` in `report_cmd` covers: `UnknownRoot`
  (Req 2.5 -> exit 2), `Metadata` (cargo metadata failed -> exit 2), and
  `WriteOutput { path }` (Req 9.4 -> exit 2). `mod.rs::run_report` maps any error
  to a `2` return and a single ASCII `eprintln!`, mirroring `main`'s existing
  fatal handling. No `unwrap`/`expect` in non-test code (NFR-6).
- A per-root `cargo tree` failure is NOT fatal: it degrades that root's closure to
  `{root}`, records the root in `degraded_roots`, and the report notes it
  (Req 3.3). Only a metadata failure (can't even list members) is fatal.
- Console output is ASCII and does not depend on a TTY (Req 10.1, 10.2); there is
  no live bar in the report path at all.

### 8.8 Testability

- `roots.rs`, `wiring.rs`, `render.rs`: pure unit tests over in-memory
  `WorkspaceFacts` and synthetic closures (AC-1..AC-9, AC-14, AC-15). No cargo
  process is spawned in these tests, so they are deterministic and fast.
- `summary.rs`: unit tests over a temp CSV fixture (AC-10, AC-11), following the
  temp-file pattern already used in `history.rs` tests.
- Path resolution (AC-12, AC-13) is a pure function
  `resolve_report_path(out, log_dir, root) -> PathBuf` tested directly.
- An integration smoke test (optional, consistent with section 6's "next")
  builds the binary and runs `cargo gate report` against a fixture workspace,
  asserting the file is written and contains the section headings.

### 8.9 Semver / contract note (Req 11)

This feature is strictly ADDITIVE: a new `report` subcommand, new report-only
flags (`--root`, `--report-out`, `--log-dir` reused), and a new output file
`gate.wiring-report.md`. No existing flag, the exit-code meanings (0/1/2), or the
`gate.history.csv` columns change (the history file is read-only here). Per
semver this is a MINOR release; `Cargo.toml` `version` goes `0.1.0 -> 0.2.0`.

## 9. Bug fix: nextest early-cancellation summary parse (Requirement 12)

Localized fix; it changes module boundaries in no way and touches NONE of the
public contract (CLI surface, exit codes, history CSV columns) -- so it is a pure
mis-parse correction, not a breaking change, and needs no semver bump.

Root cause: `nextest_summary_re()` in `src/phases/nextest.rs` matches a single
integer immediately before `tests run:`:
`Summary\s+\[.*?\]\s+(\d+)\s+tests? run:\s+(\d+)\s+passed(?:.*?(\d+)\s+failed)?`.
When nextest fast-fails it prints the fraction form `5473/9916 tests run:`, which
this pattern cannot match, so `parse_nextest_summary` returns `None`. Downstream
in `gate.rs::run_test_phase`, a `None` totals then falls back to the up-front
`cargo nextest list` count for `items_total` while leaving `items_passed`/
`items_failed` unset, which `test_count_summary` prints via `unwrap_or(0)` as the
contradictory "9916 run, 0 passed, 0 failed".

Two concrete, localized changes:

1. `nextest_summary_re()` (and thus `parse_nextest_summary`): widen the run-count
   portion to accept BOTH the plain `N tests run:` form and the fraction
   `R/T tests run:` form, capturing R (the left side of the fraction) as the run
   count. The plain form keeps R == the single number. The passed capture and the
   optional `failed` capture are unchanged, so the existing optional `, F failed`
   (defaulting to 0), trailing `, S skipped`, and parenthetical annotations such
   as `(6 slow)`/`(2 leaky)` continue to parse. The chosen regex shape makes the
   count before `tests run:` an optional `\d+/` fraction prefix followed by the
   captured run integer:
   `Summary\s+\[.*?\]\s+(?:\d+/)?(\d+)\s+tests? run:\s+(\d+)\s+passed(?:.*?(\d+)\s+failed)?`.
   This is the smallest change that captures the RUN count (not the total `T`) in
   the fraction case while leaving every already-passing case untouched.

2. `run_test_phase` None-totals guard: when `totals` is `None` AND the run did
   not succeed, do NOT borrow the up-front nextest-list total for `items_total`.
   Leaving `items_total = None` makes `test_count_summary` already print "counts
   unavailable" (it keys off `items_total`), so no change to `test_count_summary`
   is needed -- only the fallback assignment in `run_test_phase` is gated on
   success. The existing fallback to the list total stays only for the
   success-but-unparsed case (if any), preserving prior behaviour where it was
   sensible.

Testability: change 1 is covered by pure unit tests on `parse_nextest_summary`
(fraction form, plain form, skipped/annotation tolerance) in `nextest.rs`. Change
2 is covered by a `run_test_phase`-level assertion that a failed, unparsed run
yields `items_total = None` -- or, if the full phase is awkward to drive in a unit
test, by a focused test on the count-summary/guard logic reachable without
spawning cargo. `test_count_summary` already has unit tests asserting both
"counts unavailable" and the formatted-totals cases.

## 10. Default scope = Cargo default-members (Requirement 16)

Makes the gate's DEFAULT package scope follow Cargo `default-members`, with an
explicit `--workspace`/`--all` opt-in for the full member set. This extends the
scope model of sections 2 (`config`) and 3.8 (`workspace.rs`) without changing
any other decision. It is the one semver-notable default change (0.2.0 -> 0.3.0).

### 10.1 The new `Scope` variant and precedence (`config.rs`)

`Scope` gains a `DefaultMembers` variant, which becomes the resolved scope when
no explicit selector is given. `Cli` gains a `--workspace` flag with a clap
`visible_alias = "all"`, mapping to the existing `Scope::Workspace`. The existing
`--crate`, `--app-only`/`--closure-of` flags are unchanged. `Config::from_cli`
resolves scope by precedence (most specific first, Req 16.4):

```
--crate <name>            -> Scope::Crate { name }
--app-only --closure-of   -> Scope::AppOnly { closure_of }
--workspace | --all       -> Scope::Workspace
(none)                    -> Scope::DefaultMembers        // the new default
```

`Scope::label()` returns the distinct value `"default-members"` for the new
variant (Req 16.3) and `"workspace"` for `Scope::Workspace`; the history CSV
columns are untouched (NFR-2) -- only a new label VALUE joins the existing ones.
`Scope::describe()` gives the default scope its own ASCII banner text (Req 16.6).

### 10.2 The single package-selection chokepoint (Req 16.5)

`nextest::scope_args` is the ONE place that turns a `Scope` into cargo/nextest
package-selection args. It gains a `Scope::DefaultMembers => Vec::new()` arm:
emitting NO args means cargo/nextest fall back to `default-members`. Every test
path already routes through `scope_args` (`nextest_list_args`, `nextest_run_args`,
`cargo_test_args`, `cargo_build_args`).

The fmt/clippy phase previously had its OWN scope match in `clippy_args`
(`_ => --workspace`), which would have silently kept gating `--workspace` under
the new default. To satisfy Req 16.5 (consistent honouring across phases),
`clippy_args` now DELEGATES to `nextest::scope_args(scope, &[])` so clippy shares
the exact same chokepoint. Clippy never received orphan excludes (the gate never
passed them), so an empty orphan slice preserves clippy's prior AppOnly behaviour
(`--workspace` with no `--exclude`) while gaining the `DefaultMembers` (empty) and
`Crate` (`-p`) behaviours identically to the test phase. `cargo fmt --check` takes
no package selection and is unaffected.

`gate.rs` is unchanged: it already threads `config.scope` and
`config.scope.label()` through the phases and history. `resolve_orphans` returns
empty for any non-`AppOnly` scope, so `DefaultMembers` derives no orphans.

### 10.3 Contract and semver (Req 16, NFR-2)

ADDITIVE except for the one default change: new `--workspace`/`--all` flag, new
`Scope::DefaultMembers` variant, new `"default-members"` history label value. No
exit-code meaning changes; no `gate.history.csv` column changes. `Cargo.toml`
`version` moves `0.2.0 -> 0.3.0` (MINOR, semver-notable default change) with a
prominent CHANGELOG behaviour-change note.

### 10.4 Testability

All changes are covered by pure unit tests with no cargo spawn: `config.rs`
resolves the default and `--workspace`/`--all` to the expected `Scope`, asserts
precedence (AC-20, AC-22, AC-24), and the distinct label values (AC-23);
`nextest.rs` asserts `scope_args(DefaultMembers)` is empty and `Workspace` is
`["--workspace"]` (AC-21, AC-22); `fmt_clippy.rs` asserts `clippy_args` emits no
`--workspace`/`-p` for the default and `--workspace` when requested (AC-21,
AC-22). The authoritative full-suite run is the owner's manual gate.
