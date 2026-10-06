# Changelog

All notable changes to cargo-gate are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/AlanWynne/cargo-gate/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/AlanWynne/cargo-gate/releases/tag/v0.1.0
