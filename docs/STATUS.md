# cargo-gate -- Status

Current project status and handoff notes. ASCII only.

## Version

- `Cargo.toml` version: `0.3.0` (bumped from `0.2.0` for the default-scope
  behaviour change below; semver-notable MINOR bump).

## Most recent change: default scope = Cargo default-members (Requirement 16)

Branch `feat/gate-default-members` (worktree under
`.worktrees/gate-default-members`), based off `main`.

### What changed
- The DEFAULT package scope for a bare `cargo gate` is now Cargo's
  `default-members` set instead of the full workspace. The gate emits NO
  package-selection argument by default, so Cargo falls back to
  `default-members` (or the whole workspace when the manifest declares none).
- New `--workspace` flag (alias `--all`) selects the FULL workspace member set --
  the behaviour that used to be the default.
- New history `scope` label VALUE `default-members` keeps the default scope's
  per-step ETA medians separate from full-workspace (`workspace`) ETAs. The
  `gate.history.csv` COLUMNS are unchanged (public contract).
- Scope precedence: `--crate` > `--app-only` > `--workspace`/`--all` >
  default (`default-members`).

### Files touched
- `src/config.rs` -- `Scope::DefaultMembers` variant; `--workspace`/`--all`
  flag; precedence in `Config::from_cli`; `label()`/`describe()`; tests.
- `src/phases/nextest.rs` -- `scope_args` `DefaultMembers => Vec::new()` arm
  (the single package-selection chokepoint); test.
- `src/phases/fmt_clippy.rs` -- `clippy_args` delegates to `nextest::scope_args`
  so clippy honours the default consistently; tests.
- `Cargo.toml` -- version `0.2.0 -> 0.3.0`.
- `CHANGELOG.md` -- `[0.3.0]` behaviour-change note plus a back-filled `[0.2.0]`
  entry (report subcommand + nextest fraction-summary fix).
- `docs/requirements.md` -- Requirement 16 + AC-20..AC-24.
- `docs/design.md` -- section 10.
- `docs/tasks.md` -- task group 12 (done).
- `docs/STATUS.md` -- this file (new).

### Verification (agent = COMPILE-VERIFY only, per the workspace execution split)
Scoped to the gate crate, logs read back from `tools/logs/`:
- `cargo check` -- clean (`Finished dev`).
- `cargo clippy --all-targets -- -D warnings` -- clean.
- `cargo fmt --check` -- clean (no diff).
- Focused new/changed unit tests -- all pass (scope/label/precedence and the
  clippy/nextest arg tests). The authoritative full suite is the owner's manual
  `cargo gate` run.

Confirmed by effect: the default scope resolves to `Scope::DefaultMembers` and
`scope_args`/`clippy_args` emit no `--workspace`/`-p` for it, while
`--workspace`/`--all` resolve `Scope::Workspace` and emit `--workspace`.

## Prior work (context)
- `0.2.0`: `cargo gate report` workspace wiring report (additive subcommand);
  nextest early-cancellation fraction-summary parse fix.
- `0.1.0`: initial release (fmt + clippy + nextest gate, history, ETA, `--build`).

## Next / handoff
- Owner to run the authoritative full `cargo gate` on this branch before merge to
  `main` (workspace gate policy: full gate green before merge).
- The `report` subcommand feature (docs tasks 1-10) remains the larger in-flight
  work; its tasks are unchanged by this scope change.
