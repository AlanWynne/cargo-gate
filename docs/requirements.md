# cargo-gate -- Requirements: `cargo gate report` (workspace wiring report)

Acceptance criteria for the new `cargo gate report` feature. Criteria are in
EARS form (WHEN ... THE ... SHALL ...) and are numbered so tests can cite them
(e.g. `// Validates: Requirement 3.2`). This document is the source of truth for
WHAT the feature must do; the HOW is in `design.md`.

All text is ASCII-only.

## Summary

`cargo gate report` produces a Markdown document describing the dependency-graph
wiring of any Cargo workspace: which workspace members are reachable from the
project's root crates ("wired in") and which members are reachable from no root
("orphans", i.e. not yet wired in), with per-root attribution, plus a summary of
the most recent gate run. Roots default to auto-detection (every binary-producing
member plus every top-of-tree library) and can be overridden with a repeatable
`--root` flag. The feature is fully generic: it must work on ANY Cargo workspace,
not only FileForgeWorkbench.

The report measures DEPENDENCY-GRAPH wiring (a crate appears in a root's cargo
dependency tree), which is a distinct, weaker claim than FUNCTIONAL registration
(a crate is actually dispatched or registered at runtime). The report must state
this distinction so the result is not over-read.

## Non-Functional Requirements

- NFR-1 Generic: no FileForgeWorkbench-specific assumptions; works on any Cargo
  workspace discoverable via `cargo metadata`.
- NFR-2 Public contract: the CLI surface, exit-code meanings, and
  `gate.history.csv` columns are a published contract. This feature is ADDITIVE
  only; it must not change existing exit-code meanings or CSV columns.
- NFR-3 Cross-platform: Windows and Unix. Code is run mainly on Windows.
- NFR-4 Lean dependencies: reuse crates already present (`serde`, `serde_json`,
  `anyhow`, `clap`); add no new dependency for this feature.
- NFR-5 Reuse: the feature reuses the existing `workspace` module machinery
  (`member_names`, `workspace_root`, `dependency_closure`, `derive_orphans`)
  rather than re-implementing `cargo metadata` / `cargo tree` parsing.
- NFR-6 Rust standards: `thiserror` for any new library error type, no
  `unwrap`/`expect` in non-test code, newtypes over bare primitives where they
  clarify intent, `#[derive(Debug)]` on public types, rustfmt clean, clippy clean
  under `-D warnings`, no source file over 400 non-test lines.

## Requirement 1 -- The `report` subcommand

The feature is invoked as a subcommand so the wiring picture is available
without running a full gate.

- 1.1 WHEN the tool is invoked as `cargo gate report` THE tool SHALL run a
  metadata-and-tree analysis only (no fmt, no clippy, no build, no test) and
  produce a Markdown wiring report.
- 1.2 WHEN invoked with no subcommand (`cargo gate ...`) THE tool SHALL behave
  exactly as before (run the gate), so existing behaviour and the public CLI
  contract are preserved (NFR-2).
- 1.3 WHEN the `report` subcommand completes successfully THE tool SHALL exit 0;
  WHEN it fails due to a usage error or an unrecoverable analysis error (e.g.
  `cargo metadata` cannot be run) THE tool SHALL exit 2, consistent with the
  existing exit-code contract (0 success, 2 usage/fatal). The `report`
  subcommand SHALL NOT introduce any new exit-code meaning.
- 1.4 WHEN the analysis runs THE tool SHALL discover the workspace via the
  existing `workspace` module (`workspace_root`, `member_names`) rather than a
  new metadata parser (NFR-5).

## Requirement 2 -- Root detection and override

- 2.1 WHEN no `--root` flag is given THE tool SHALL auto-detect the set of roots.
- 2.2 WHEN auto-detecting THE tool SHALL include every workspace member that
  produces at least one binary target.
- 2.3 WHEN auto-detecting THE tool SHALL additionally include every library
  member that no other workspace member depends on (a "top-of-tree" library).
- 2.4 WHEN one or more `--root <crate>` flags are given THE tool SHALL use
  exactly that explicit list as the roots and SHALL NOT auto-detect.
- 2.5 WHEN a `--root <crate>` value names a crate that is not a workspace member
  THE tool SHALL fail with a usage error (exit 2) naming the unknown crate,
  rather than silently ignoring it.
- 2.6 WHEN the report is produced THE report SHALL state, for the root set,
  whether the roots were auto-detected or user-specified.
- 2.7 WHEN auto-detection yields no roots (e.g. a workspace of only
  interdependent libraries with a single top) THE tool SHALL still select the
  top-of-tree library members so the root set is never empty for a non-empty
  workspace; WHEN the workspace has no members THE tool SHALL report an empty
  workspace rather than erroring.

## Requirement 3 -- Wired-in computation (union closure)

- 3.1 WHEN roots are known THE tool SHALL compute, for each root, that root's
  normal-edge dependency closure restricted to workspace members, reusing the
  existing `dependency_closure` logic (NFR-5).
- 3.2 WHEN per-root closures are known THE tool SHALL define the WIRED-IN set as
  the UNION of all roots' closures: a workspace member in ANY root's closure is
  wired in.
- 3.3 WHEN a `cargo tree` closure for a given root cannot be derived THE tool
  SHALL record that root's closure as containing only the root itself and SHALL
  note the degraded closure in the report, rather than aborting the whole report.

## Requirement 4 -- Orphan derivation

- 4.1 WHEN the wired-in set is known THE tool SHALL define ORPHANS as the
  workspace members that appear in NO root's closure.
- 4.2 WHEN the orphan set is computed THE tool SHALL exclude the roots themselves
  from the orphan set (a root is wired in by definition).
- 4.3 WHEN there are no orphans THE report SHALL state explicitly that every
  workspace member is wired in, rather than omitting the orphan section.
- 4.4 WHEN orphan and wired-in sets are rendered THE tool SHALL sort member names
  deterministically (lexicographic) so repeated runs on an unchanged workspace
  produce identical output.

## Requirement 5 -- Per-root attribution

- 5.1 WHEN a wired-in crate is listed THE report SHALL show which root(s) pull it
  in (the roots whose closure contains that crate).
- 5.2 WHEN a crate is in more than one root's closure THE report SHALL list all
  attributing roots for that crate.

## Requirement 6 -- Honesty label

- 6.1 WHEN the report is produced THE report SHALL contain a clearly visible
  statement that it measures dependency-graph wiring (crate present in a root's
  dependency tree), NOT functional registration (actually dispatched/registered
  at runtime), and that a declared dependency can still be functionally
  unreachable.

## Requirement 7 -- Report contents and sections

The Markdown report SHALL contain, in order, the following sections.

- 7.1 A "Gate run summary" section: the latest gate verdict, per-phase timing,
  and test counts (see Requirement 8 for sourcing).
- 7.2 A "Roots" section listing the detected or declared roots and whether they
  were auto-detected or user-specified (Requirement 2.6).
- 7.3 A "Wired-in crates" section listing each wired-in crate with its
  attributing root(s) (Requirement 5).
- 7.4 An "Orphan crates" section under its own heading, clearly separated from
  the wired-in crates (Requirement 4).
- 7.5 A "Dependency tree" section showing a tree view rooted at each root crate,
  reusing the cargo tree / closure data.
- 7.6 The honesty label of Requirement 6 SHALL appear in the report (at or near
  the wired-in / orphan sections so it is read in context).

## Requirement 8 -- Gate run summary sourcing (history)

- 8.1 WHEN `cargo gate report` runs as a standalone subcommand THE tool SHALL
  source the gate run summary (verdict, per-phase timing, test counts) from the
  most recent run recorded in `gate.history.csv` under the active `--log-dir`,
  reading the existing long-format rows (including the `overall` row) WITHOUT
  modifying the CSV or its columns (NFR-2).
- 8.2 WHEN the history file exists and has at least one complete run THE summary
  SHALL reflect that most recent run's rows (identified by its `run_id`).
- 8.3 WHEN no history file exists, or it contains no runs THE report SHALL state
  that no prior gate run is recorded and SHALL still produce the wiring sections
  (roots, wired-in, orphans, tree), so the wiring picture is available before any
  gate has ever run.
- 8.4 WHEN the history rows for the latest run do not include a particular field
  (e.g. test counts absent) THE summary SHALL render that field as unavailable
  rather than fabricating a value.

## Requirement 9 -- Output path and override

- 9.1 WHEN no output-path override is given THE tool SHALL write the Markdown
  report to a default path under the active `--log-dir` (default `.gate/`),
  named `gate.wiring-report.md`.
- 9.2 WHEN an output-path override flag is given THE tool SHALL write the report
  to that path instead, creating parent directories as needed.
- 9.3 WHEN the report is written THE tool SHALL also print a short ASCII
  confirmation line naming the written path.
- 9.4 WHEN the output file cannot be written (e.g. permission denied) THE tool
  SHALL fail with exit 2 and an error naming the path, rather than exiting 0.

## Requirement 10 -- Output and portability constraints

- 10.1 WHEN the tool prints to the console THE console output SHALL be ASCII only.
- 10.2 WHEN stdout is not a TTY THE tool SHALL still run to completion and
  produce the report and confirmation line (no interactive or TTY-only
  behaviour), consistent with the gate's existing quiet-mode behaviour.
- 10.3 WHEN run on Windows or Unix THE tool SHALL behave identically with respect
  to analysis and report content (portable paths, no shell-specific assumptions).

## Requirement 11 -- Public-contract / semver impact

- 11.1 WHEN this feature is released THE change SHALL be ADDITIVE to the public
  contract: a new `report` subcommand and new report-only flags, with no change
  to existing flags, exit-code meanings, or `gate.history.csv` columns, and SHALL
  be released as a MINOR version bump (per semver for an additive public-API
  change).

## Acceptance Criteria (testable, numbered)

- AC-1 (Req 2.2, 2.3) Given a synthetic member/target/dependency model with one
  binary member `app`, a library `lib-top` that nothing depends on, and a library
  `lib-mid` that `app` depends on, auto-detection returns roots `{app, lib-top}`
  and not `lib-mid`.
- AC-2 (Req 2.4) Given explicit `--root app`, root detection returns exactly
  `{app}` regardless of what auto-detection would have produced.
- AC-3 (Req 2.5) Given `--root does-not-exist` where that crate is not a member,
  root resolution returns an error (mapped to exit 2).
- AC-4 (Req 3.2) Given roots `{app, ffmdx}` with closures `{app, lib-a}` and
  `{ffmdx, lib-a, lib-b}`, the wired-in set is `{app, ffmdx, lib-a, lib-b}`.
- AC-5 (Req 4.1, 4.2) Given the AC-4 model plus member `orphan-x` in no closure,
  orphan derivation returns exactly `{orphan-x}`.
- AC-6 (Req 5.1, 5.2) Given the AC-4 model, attribution for `lib-a` lists both
  `app` and `ffmdx`; attribution for `lib-b` lists only `ffmdx`.
- AC-7 (Req 4.4) Two runs over the same model produce byte-identical wired-in and
  orphan listings (sorted).
- AC-8 (Req 6.1, 7.6) The rendered Markdown contains the honesty-label text
  distinguishing dependency-graph wiring from functional registration.
- AC-9 (Req 7.1-7.5) The rendered Markdown contains the five section headings in
  order: gate run summary, roots, wired-in crates, orphan crates, dependency tree.
- AC-10 (Req 8.3) With no history file present, the rendered Markdown states that
  no prior gate run is recorded and still contains the wiring sections.
- AC-11 (Req 8.2) Given a history CSV whose latest run_id has a CLEAN verdict and
  known timings, the gate-run-summary section reflects that verdict and those
  timings.
- AC-12 (Req 9.1) With no override, the report path resolves to
  `<log-dir>/gate.wiring-report.md`.
- AC-13 (Req 9.2) With an override flag, the report path resolves to the override.
- AC-14 (Req 4.3) With no orphans, the orphan section states that every member is
  wired in.
- AC-15 (Req 2.6) The roots section states "auto-detected" when no `--root` was
  given and "user-specified" when `--root` was given.

## Requirement 12 -- Nextest summary parsing (bug fix: early-cancellation fraction form)

BUG CONTEXT: a real `cargo gate` run against another workspace printed the
contradictory verdict line
`cargo-gate: ISSUES -- see <path>/.gate/gate.review.log (9916 run, 0 passed, 0 failed)`.
The real nextest summary line was
`Summary [ 202.706s] 5473/9916 tests run: 5472 passed, 1 failed, 0 skipped`.
When nextest cancels early on a test failure (fast-fail), it prints a
`R/T tests run:` FRACTION (e.g. `5473/9916`) instead of a single `N tests run:`
count. The summary regex in `nextest_summary_re()` expects a single number
immediately before `tests run:`, so it fails to match the fraction form,
`parse_nextest_summary` returns `None`, and `run_test_phase` then falls back to
the up-front `cargo nextest list` total (9916) for `items_total` while
`items_passed`/`items_failed` stay unset and render via `unwrap_or(0)` as
"0 passed, 0 failed" -- the nonsensical line.

This is a mis-parse CORRECTION. It changes NONE of the public contract: no change
to the CLI surface, the exit-code meanings (0/1/2), or the `gate.history.csv`
columns. It is therefore NOT a breaking change and requires NO semver bump
(per NFR-2 / Requirement 11.1 this is a pure bug fix within the existing
contract).

- 12.1 WHEN nextest prints the early-cancellation fraction summary form
  (`Summary [ ... ] R/T tests run: P passed, F failed[, S skipped]`) THE tool
  SHALL parse the RUN count as the LEFT side of the fraction (R) and SHALL still
  extract the passed count (P) and the failed count (F) correctly.
- 12.2 WHEN the plain non-cancelled summary form
  (`Summary [ ... ] N tests run: P passed[, F failed][, S skipped]`) is printed
  THE tool SHALL continue to parse N as the run count, P as passed, and F as
  failed, exactly as before this fix (no regression).
- 12.3 WHEN the summary line omits the `, F failed` clause THE tool SHALL default
  the failed count to 0, and WHEN the summary line carries a trailing
  `, S skipped` clause or parenthetical annotations such as `(6 slow)` or
  `(2 leaky)` THE tool SHALL still parse the run, passed, and failed counts
  correctly (these forms must continue to parse).
- 12.4 WHEN the final test-summary parse fails (returns no totals) AND the test
  run did not succeed THE tool SHALL report the test counts as "counts
  unavailable" and SHALL NOT fabricate a "0 passed, 0 failed" line from the
  up-front `cargo nextest list` total; i.e. it SHALL NOT populate `items_passed`
  / `items_failed` as 0 against a borrowed `items_total`.

### Acceptance Criteria for Requirement 12

- AC-16 (Req 12.1) `parse_nextest_summary` on the line
  `Summary [ 202.706s] 5473/9916 tests run: 5472 passed, 1 failed, 0 skipped`
  returns `TestTotals { run: 5473, passed: 5472, failed: 1 }`.
- AC-17 (Req 12.2) `parse_nextest_summary` on
  `Summary [ 12.345s] 5000 tests run: 4998 passed, 2 failed` still returns
  `TestTotals { run: 5000, passed: 4998, failed: 2 }` (existing behaviour).
- AC-18 (Req 12.3) `parse_nextest_summary` on
  `Summary [ 1.00s] 10 tests run: 10 passed, 0 skipped` returns
  `TestTotals { run: 10, passed: 10, failed: 0 }` (failed defaults to 0; trailing
  skipped tolerated); and on a line carrying a `(6 slow)` / `(2 leaky)`
  parenthetical annotation the run/passed/failed counts still parse.
- AC-19 (Req 12.4) Given a test run that did NOT succeed and whose summary did
  not parse (totals `None`), the test `StepResult` carries `items_total = None`
  (not the borrowed nextest-list total) so `test_count_summary` reports
  "counts unavailable" rather than "<list-total> run, 0 passed, 0 failed".

## Requirement 16 -- Default scope = Cargo default-members (with --workspace/--all opt-in)

CONTEXT: on a large workspace where only a fraction of crates are wired into the
product, gating the whole workspace on every change is slow. Cargo already has a
first-class mechanism for "the crates that matter by default": the manifest's
`default-members`. This requirement makes the gate's DEFAULT scope follow Cargo
`default-members`, and adds an explicit flag to request the full workspace.

This is a DEFAULT-BEHAVIOUR change and is semver-notable: a bare `cargo gate`
that previously gated `--workspace` now gates `default-members`. It is ADDITIVE
in every other respect (a new flag, a new history label value) and changes NO
exit-code meaning and NO `gate.history.csv` column (NFR-2). Released as a MINOR
bump (`0.2.0 -> 0.3.0`).

- 16.1 WHEN the tool is invoked with no explicit scope flag (`--crate`,
  `--app-only`, `--workspace`/`--all`) THE gate SHALL select the DEFAULT scope,
  which emits NO package-selection argument (no `--workspace`, no `-p`,
  no `--exclude`) to any phase (fmt, clippy, nextest list/run, cargo test, and
  the optional build), so Cargo falls back to the workspace `default-members`
  set (or the whole workspace when the manifest declares no `default-members`).
- 16.2 WHEN the tool is invoked with `--workspace` (or its alias `--all`) THE
  gate SHALL select the FULL workspace member set (emit `--workspace`), the
  behaviour that was the default before this change.
- 16.3 WHEN history rows are written for a DEFAULT-scope run THE gate SHALL
  record the `scope` column with the distinct label value `default-members`, so
  per-scope ETA medians for the default scope do not mix with full-workspace
  (`workspace`) ETAs. THE set of `gate.history.csv` COLUMNS SHALL be unchanged
  (public contract); only a new label VALUE is introduced.
- 16.4 WHEN more than one scope selector is supplied THE gate SHALL apply the
  precedence `--crate` > `--app-only` > `--workspace`/`--all` > DEFAULT
  (`default-members`): the most specific selector wins.
- 16.5 WHEN the DEFAULT scope is active THE clippy, fmt, and test phases SHALL
  all honour it consistently (a single package-selection chokepoint), so none of
  them silently gates `--workspace` while another gates `default-members`.
- 16.6 WHEN console output names the active scope THE output SHALL remain ASCII
  and SHALL describe the default scope distinctly from the full workspace.

### Acceptance Criteria for Requirement 16

- AC-20 (Req 16.1) With no scope flag, `Config::from_cli` resolves
  `Scope::DefaultMembers`.
- AC-21 (Req 16.1) `scope_args(&Scope::DefaultMembers, &[])` returns an empty
  vector (no `-p`, no `--workspace`), and `clippy_args(&Scope::DefaultMembers)`
  contains neither `--workspace` nor `-p`.
- AC-22 (Req 16.2) `--workspace` and `--all` both resolve `Scope::Workspace`;
  `scope_args(&Scope::Workspace, &[])` returns `["--workspace"]` and
  `clippy_args(&Scope::Workspace)` contains `--workspace`.
- AC-23 (Req 16.3) `Scope::DefaultMembers.label()` is `"default-members"` and
  `Scope::Workspace.label()` is `"workspace"` (distinct values; same columns).
- AC-24 (Req 16.4) Given `--workspace --crate x`, the resolved scope is
  `Crate { x }`; given `--workspace --app-only --closure-of app`, it is
  `AppOnly { app }` (the more specific selector wins over `--workspace`).

## Out of Scope

- Functional-registration analysis (whether a crate is actually dispatched or
  registered at runtime). The honesty label explicitly disclaims this; detecting
  it is a separate, larger effort.
- Changing existing gate behaviour, flags, exit codes, or CSV columns.
- Non-normal dependency edges (dev-dependencies, build-dependencies) in the
  wired-in computation; the closure uses normal edges only, matching the existing
  `dependency_closure`.
- HTML/other export formats; the report is Markdown only in this cut.
- Attaching the report to a live gate run as a bolt-on phase (the standalone
  subcommand is the chosen primary form; see design for rationale).
