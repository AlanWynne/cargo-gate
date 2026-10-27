# Changelog

All notable changes to cargo-gate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0]

### Changed (behaviour change -- read before upgrading)
- DEFAULT SCOPE is now Cargo's `default-members` set, NOT the full workspace.
  A bare `cargo gate` emits no package selection (no `--workspace`, no `-p`), so
  Cargo gates the manifest's `default-members` (or the whole workspace when the
  manifest declares none). Previously a bare run always gated `--workspace`. This
  lets a large workspace narrow its everyday gate to the crates that matter while
  keeping the full set one flag away. This is the only behaviour change and the
  reason for the MINOR version bump (semver-notable default change).

### Added
- `--workspace` flag (alias `--all`) selects the FULL workspace member set --
  the behaviour that used to be the default. Use it for the authoritative
  whole-workspace gate.
- A distinct history `scope` label value `default-members` so the default
  scope's per-step ETA medians do not mix with full-workspace ETAs. The
  `gate.history.csv` COLUMNS are unchanged (public contract); only a new label
  VALUE is introduced.

### Notes
- Scope precedence is `--crate` > `--app-only` > `--workspace`/`--all` >
  default (`default-members`).
- `--crate`, `--app-only`/`--closure-of`, `--build`, `--release`, `--fast`, and
  the `report` subcommand are unchanged. Exit-code meanings (0 CLEAN / 1 ISSUES /
  2 TIMEOUT|fatal) are unchanged.

## [0.2.0]

### Added
- `cargo gate report`: a workspace dependency-graph wiring report (which members
  are wired in from the project roots and which are orphans), written as Markdown
  with per-root attribution and an honesty label distinguishing dependency-graph
  wiring from functional registration. Additive subcommand and flags; no change
  to existing gate behaviour, exit codes, or history CSV columns.

### Fixed
- Nextest early-cancellation summary parsing: the fast-fail `R/T tests run:`
  fraction form (e.g. `5473/9916 tests run:`) now parses the run count (R),
  passed, and failed correctly instead of falling back to a contradictory
  "0 passed, 0 failed" line.

## [0.1.0] - 2026-10-06

Initial release.

### Added
- `cargo gate`: runs `cargo fmt --check`, `cargo clippy`, and the test phase
  (`cargo nextest run`, falling back to `cargo test`) as one verification gate
  with a single CLEAN / ISSUES / TIMEOUT verdict and matching exit code.
- Structured parsing over text scraping: clippy diagnostics via
  `--message-format=json`, the test total via `cargo nextest list
  --message-format json`, and live test events via the experimental libtest-json
  stream with a human `(n/N)` regex fallback.
- Build vs test timing split (compiling the test binaries vs running them),
  timed and tracked separately.
- Scopes: default `--workspace`, `--crate <name>`, and `--app-only --closure-of
  <crate>` (workspace minus the crates outside that binary's dependency closure,
  derived at runtime so the exclude list never drifts).
- `--fast` mode (`PROPTEST_CASES=32`) for a quick signal.
- Durable artifacts under `--log-dir` (default `./.gate`): a filtered combined
  report, a review log (empty == clean), a per-step timing log, a long-format
  per-step history CSV, and raw per-step stdout/stderr.
- Overall run ETA and completion variance: a start header estimates completion
  time from history, and a completion footer reports the actual duration and its
  signed variance from the estimate. The headline ETA is learned from whole-run
  history so it tracks real end-to-end time.
- `--build` (and `--build <bin>`, composable with `--release`): an opt-in build
  phase after a clean gate that leaves a runnable binary. Skipped automatically
  when the gate is not clean.
- Per-step watchdog with a configurable `--step-timeout`.
- Print-on-completion console output: one line per phase as it finishes, with no
  live progress bar, so output is clean on any terminal and when redirected.

### Notes
- Developed and run primarily on Windows; written to be cross-platform.
- Dual licensed under MIT OR Apache-2.0.

[Unreleased]: https://github.com/AlanWynne/cargo-gate/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/AlanWynne/cargo-gate/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/AlanWynne/cargo-gate/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/AlanWynne/cargo-gate/releases/tag/v0.1.0
