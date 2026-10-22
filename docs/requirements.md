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
