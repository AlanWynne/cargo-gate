# cargo-gate -- Tasks: `cargo gate report` (workspace wiring report)

Ordered implementation tasks for the `cargo gate report` feature. Checkbox
markers are `[ ]` (pending) or `[x]` (done) only. Each task cross-references the
requirement(s) it satisfies. Tasks follow TDD: failing tests are written BEFORE
the implementation that satisfies them, and each test carries a
`// Validates: Requirement X.Y` annotation. Agents verify by scoped
compile/clippy/fmt and the scoped new tests; the owner runs the full gate.

- [ ] 1. Extend workspace discovery with target/dependency facts
  - [ ] 1.1 Write failing unit tests in `workspace.rs` for a pure
    `facts_from_metadata` parser: given a `cargo metadata --no-deps` JSON
    fixture, it returns one `MemberFact` per member with correct `produces_bin`
    and `depends_on`. (Req 2.2, 2.3, NFR-5)
  - [ ] 1.2 Add `WorkspaceFacts`, `MemberFact`, and the `CrateName` newtype;
    extend the `Package`/`Target`/`Dep` serde structs; implement
    `facts_from_metadata` (pure) and `pub(crate) fn workspace_facts(cargo, cwd)`
    reusing the single existing `cargo metadata` call. Derive `Debug`. (Req 1.4,
    2.2, 2.3, NFR-5, NFR-6)
  - [ ] 1.3 Make `dependency_closure` `pub(crate)` with no behaviour change;
    confirm the existing orphan test still passes. (Req 3.1, 3.3, NFR-5)

- [ ] 2. Root detection and override (`report_cmd/roots.rs`, pure)
  - [ ] 2.1 Write failing tests: auto-detect returns binaries plus top-of-tree
    libs and excludes depended-on mid libs (AC-1); explicit `--root` replaces
    auto-detection (AC-2); unknown `--root` crate yields an error (AC-3);
    empty-member workspace yields empty roots with the empty-workspace note
    (Req 2.7). (Req 2.2-2.5, 2.7)
  - [ ] 2.2 Implement `resolve_roots(facts, overrides) -> Result<Roots>` and the
    `RootProvenance` enum to make the tests pass. (Req 2.1-2.7, 2.6/AC-15)

- [ ] 3. Union-closure, orphan derivation, attribution (`report_cmd/wiring.rs`, pure)
  - [ ] 3.1 Write failing tests: union of per-root closures is the wired-in set
    (AC-4); orphans are members in no closure and exclude roots (AC-5);
    attribution lists all roots that pull a crate in (AC-6); two runs produce
    identical sorted output (AC-7). (Req 3.2, 4.1, 4.2, 4.4, 5.1, 5.2)
  - [ ] 3.2 Implement `compute_wiring(facts, roots, closures) -> Wiring` with the
    `Wiring` data model (BTree collections, `degraded_roots`), making the tests
    pass. (Req 3.2, 3.3, 4.1-4.4, 5.1, 5.2, NFR-6)

- [ ] 4. Gate run summary from history (`report_cmd/summary.rs`)
  - [ ] 4.1 Write failing tests over a temp `gate.history.csv` fixture: latest
    run_id with a CLEAN verdict and known timings yields the expected
    `RunSummary` (AC-11); absent/empty history yields `None` (AC-10); missing
    test-count columns render as unavailable (Req 8.4). (Req 8.1, 8.2, 8.4)
  - [ ] 4.2 Implement the read-only CSV reader and
    `latest_run_summary(path) -> Option<RunSummary>`, re-deriving the verdict with
    the same fold rule as `Verdict::from_results`; reuse `report::format_hms`. Do
    not modify `history.rs`. (Req 8.1-8.4, NFR-2)

- [ ] 5. Markdown rendering (`report_cmd/render.rs`, pure)
  - [ ] 5.1 Write failing tests: the honesty label text is present (AC-8); the
    five section headings appear in order (AC-9); no-orphans states every member
    wired in (AC-14); roots section states auto-detected vs user-specified
    (AC-15); "no prior gate run recorded" note appears when summary is `None`
    (AC-10); output is ASCII only (Req 10.1). (Req 6, 7.1-7.6, 4.3, 8.3, 10.1)
  - [ ] 5.2 Implement `render_report(&Wiring, &Option<RunSummary>, &RootProvenance)
    -> String` as a pure function producing the full ASCII Markdown document,
    including the per-root dependency-tree section from the retained closures.
    (Req 6, 7.1-7.6, NFR-6)

- [ ] 6. CLI subcommand wiring (`config.rs`, `main.rs`)
  - [ ] 6.1 Write failing tests: parsing `cargo gate` (no subcommand) still
    yields the gate path/config unchanged (Req 1.2); parsing
    `cargo gate report --root a --root b --report-out x.md` yields the report
    command with both roots and the override; `report` with no `--root` carries
    an empty root list (auto-detect). (Req 1.1, 1.2, 2.4, 9.2)
  - [ ] 6.2 Add the optional `command: Option<Command>` subcommand and
    `ReportArgs` to `Cli`; resolve a `ReportConfig`; keep `strip_subcommand_token`
    and all existing flags unchanged. (Req 1.1, 1.2, 9.1, 9.2, 11.1)
  - [ ] 6.3 Add `mod report_cmd;` and dispatch in `main.rs`:
    `Some(Command::Report) -> report_cmd::run_report` mapped to `ExitCode`
    (0 success, 2 fatal/usage); `None` -> existing `gate::run`. (Req 1.1, 1.3)

- [ ] 7. Report entry point and path resolution (`report_cmd/mod.rs`)
  - [ ] 7.1 Write failing tests for `resolve_report_path(out, log_dir, root)`:
    default resolves to `<log-dir>/gate.wiring-report.md` (AC-12); override is
    honoured (AC-13). (Req 9.1, 9.2)
  - [ ] 7.2 Implement `run_report(&ReportConfig) -> Result<i32>`: gather
    `workspace_facts`, resolve roots, compute per-root closures (degrading on
    `cargo tree` failure), build the `Wiring`, read the latest `RunSummary`,
    render the Markdown, and write it to the resolved path with a confirmation
    line; add the `ReportError` (`thiserror`) enum and map errors to exit 2.
    (Req 1.1, 1.3, 1.4, 3.1, 3.3, 8.1, 9.1-9.4, 10.1, 10.2)

- [ ] 8. File-size, lint, and contract conformance
  - [ ] 8.1 Confirm every new/changed source file is under 400 non-test lines;
    split a module further by concern if needed. (rust-standards)
  - [ ] 8.2 Run scoped `cargo fmt --check`, `cargo clippy -- -D warnings`, and the
    new scoped unit tests; read the logs and fix any issue. (NFR-6)
  - [ ] 8.3 Bump `Cargo.toml` version `0.1.0 -> 0.2.0` and note the additive
    subcommand/flags/output-file in the README/changelog. (Req 11.1)

- [ ] 9. Integration smoke test (optional, consistent with design section 6)
  - [ ] 9.1 Add a fixture-workspace integration test that runs the built binary
    as `cargo gate report`, asserting the report file is written and contains the
    five section headings and the honesty label. (Req 1.1, 7.1-7.6, 9.1)

- [ ] 10. Hand off for the owner's full gate
  - [ ] 10.1 Summarise criteria implemented, files changed, commands run, and
    results; request the owner's full `cargo gate` run as the authoritative
    green check. (testing split)

- [x] 12. Default scope = Cargo default-members, with --workspace/--all opt-in (Requirement 16)
  - [x] 12.1 Update the three tests that encode the old default and add the new
    ones BEFORE/with the change: `config::default_scope_is_full_workspace` ->
    `default_scope_is_default_members` (AC-20); `nextest::scope_args` default-test
    -> `scope_args_for_default_members_is_empty` (AC-21);
    `fmt_clippy::clippy_args_use_workspace_by_default` ->
    `clippy_args_default_members_emit_no_workspace` (AC-21). Add tests for
    `--workspace`/`--all` selecting the full set (AC-22), the distinct label
    values (AC-23), and precedence (AC-24). Each carries a
    `// Validates: Requirement 16.x` annotation. (Req 16.1-16.4)
  - [x] 12.2 Add `Scope::DefaultMembers`; update `Scope::label()`
    ("default-members") and `Scope::describe()`; add the `--workspace`
    flag with `visible_alias = "all"`; set the precedence in `Config::from_cli`
    (`--crate` > `--app-only` > `--workspace`/`--all` > default). (Req 16.1, 16.2,
    16.3, 16.4, 16.6)
  - [x] 12.3 Add the `Scope::DefaultMembers => Vec::new()` arm to
    `nextest::scope_args` (the single chokepoint) so no `-p`/`--workspace` is
    emitted. (Req 16.1, 16.5)
  - [x] 12.4 Make `clippy_args` delegate to `nextest::scope_args(scope, &[])` so
    fmt/clippy/nextest honour the default consistently (no silent `--workspace`).
    (Req 16.5)
  - [x] 12.5 Bump `Cargo.toml` `0.2.0 -> 0.3.0`; update `CHANGELOG.md`
    [Unreleased]/[0.3.0] with the behaviour change, the new flag, and the new
    history label value; update `docs/requirements.md` (Req 16), `docs/design.md`
    (section 10), and create `docs/STATUS.md`. (Req 16, NFR-2)
  - [x] 12.6 Compile-only verification (agent): scoped `cargo check` (clean),
    `cargo clippy --all-targets -- -D warnings` (clean), `cargo fmt --check`
    (clean), and the focused new/changed unit tests (all pass); logs read back.
    Full suite left to the owner's manual gate run. Confirmed default mode emits
    no `--workspace`/no `-p` and `--workspace`/`--all` select the full set.
    (NFR-6, testing split)

- [x] 11. Bug fix: nextest early-cancellation summary parse (Requirement 12)
  - [x] 11.1 Write failing unit tests in `nextest.rs` for `parse_nextest_summary`
    BEFORE touching the regex: the fraction form
    `Summary [ 202.706s] 5473/9916 tests run: 5472 passed, 1 failed, 0 skipped`
    yields `TestTotals { run: 5473, passed: 5472, failed: 1 }` (AC-16); the plain
    form `5000 tests run: 4998 passed, 2 failed` still yields run 5000 / passed
    4998 / failed 2 (AC-17); a `10 tests run: 10 passed, 0 skipped` line yields
    failed 0 and a line with a `(6 slow)`/`(2 leaky)` annotation still parses
    (AC-18). Each test carries its `// Validates: Requirement 12.x` annotation.
    (Req 12.1, 12.2, 12.3)
  - [x] 11.2 Widen `nextest_summary_re()` to capture the RUN count and optionally
    consume the `/T` total suffix (`(\d+)(?:/\d+)?\s+tests? run:`) so the run
    count is the LEFT side of the fraction, leaving the passed/optional-failed
    captures unchanged; confirm the new and existing `parse_nextest_summary`
    tests pass. (Req 12.1, 12.2, 12.3; AC-16, AC-17, AC-18)
    - NOTE: first attempt `(?:\d+/)?(\d+)` captured the RIGHT side (T=9916);
      corrected to `(\d+)(?:/\d+)?` to capture R=5473.
  - [x] 11.3 Write a failing test for the counts-unavailable guard: a test run
    that did NOT succeed with unparsed totals (`None`) must yield a test
    `StepResult` with `items_total = None` so `test_count_summary` reports
    "counts unavailable" rather than borrowing the nextest-list total. Driven at
    the pure-helper level (no cargo spawn): `fallback_items_total` unit tests plus
    an end-to-end `test_count_summary` assertion. (Req 12.4; AC-19)
  - [x] 11.4 In `gate.rs::run_test_phase`, gate the `final_progress.total`
    fallback for `items_total` on run success (extracted to the pure
    `fallback_items_total(run_succeeded, list_total)`) so a failed, unparsed run
    leaves `items_total = None`; the guard test and the existing
    `test_count_summary` tests pass. (Req 12.4; AC-19)
  - [x] 11.5 Compile-only verification (agent): scoped `cargo fmt --check`
    (clean), `cargo clippy -p cargo-gate -- -D warnings` (clean), and the scoped
    new unit tests (6 parser + 3 guard, all pass); logs read back. Full suite
    left to the owner's manual gate run. Confirmed no public-contract change
    (CLI, exit codes, history CSV columns) and NO semver bump. (NFR-2, Req 11.1,
    testing split)
