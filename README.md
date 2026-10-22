# cargo-gate

A unified verification **gate** for any Cargo project. One command runs the three
checks that matter before you call a change done -- formatting, lint, and tests --
renders them as one clean summary, and writes durable log artifacts plus a
per-step timing history so you can watch each phase trend over time and estimate
how long a run will take.

```
cargo gate
```

```
started 2026-10-06 14:45:14   estimated completion ~14:49:26 (ETA 0:04:12 from history)

cargo-gate: mode=FULL runner=nextest scope=workspace (full completion gate)

fmt       [done] 0:00:15  ok
clippy    [done] 0:00:16  clean
build     [done] 0:00:10
test      [done] 9613/9613 passed  0:03:57

completed 2026-10-06 14:52:53   actual 0:07:38   ETA was 0:04:12   variance +0:03:26 (+81.7%)

cargo-gate: CLEAN -- 9613 run, 9613 passed, 0 failed in 0:07:38. See ./.gate/gate.report.log
```

Each phase prints one line when it finishes (no flickering live bar). The run is
bracketed by a start/ETA header and a completion/variance footer so you can see,
up front, roughly when it will finish, and afterward how close that estimate was.

## Why

`cargo fmt --check`, `cargo clippy`, and `cargo nextest run` each print their own
progress and summary in their own format. Running them in sequence by hand (or in
a shell script) means three disjoint outputs, no combined pass/fail verdict, and
no record of how long each phase took last time. `cargo-gate` runs all three,
shows one coherent progress view, decides a single verdict, and keeps a history.

It is a drop-in general tool: it works on any Cargo workspace, not a specific
project.

## What it does

1. `cargo fmt --check` -- formatting (no compile)
2. `cargo clippy` -- compile-check plus lint, diagnostics parsed from JSON
3. `cargo nextest run` -- builds the test binaries (the **build** phase) then runs
   them (the **test** phase), timed separately. Falls back to `cargo test` when
   nextest is not installed.

### Outputs (under `--log-dir`, default `./.gate/`)

| File | Contents |
|------|----------|
| `gate.report.log` | Combined, filtered run log: section headers, group/summary outcomes, every warning and error, and the final verdict. Per-test "ok"/"PASS" lines and blank lines are dropped. |
| `gate.review.log` | Only errors, warnings, and failures. An **empty file means a clean gate**. |
| `gate.history.csv` | Long-format, one row **per step per run**: `run_id,timestamp,mode,scope,step,duration_secs,status,warnings,errors,items_total,items_passed,items_failed`. Drives per-phase ETA and drift tracking. |
| `gate.timing.log` | Per-step timing for the run. |
| `raw/*.log` | Raw unfiltered stdout/stderr per step. |

## Scopes

```
cargo gate                      # full workspace -- the completion gate
cargo gate --fast               # PROPTEST_CASES=32 quick signal (NOT the full gate)
cargo gate --crate ff-keys      # a single crate (inner loop)
cargo gate --app-only --closure-of my-app
                                # workspace minus crates not in my-app's dep closure
```

- `--app-only` tests only the dependency closure of the binary named by
  `--closure-of`, excluding workspace "orphan" crates not wired into it. The
  exclude list is derived at runtime (all members minus the closure) so it never
  drifts.

## Wiring report (`cargo gate report`)

```
cargo gate report                       # auto-detect roots, write the report
cargo gate report --root my-app         # explicit root(s); repeat --root for more
cargo gate report --report-out wiring.md
cargo gate report --log-dir .gate
```

`cargo gate report` writes a Markdown report of the workspace's dependency-graph
wiring without running the gate. It is fully generic (any Cargo workspace) and
multi-root:

- **Roots** default to auto-detection: every workspace member that produces a
  binary, plus every top-of-tree library (a library no other member depends on).
  Override with one or more `--root <crate>` flags; an explicit list replaces
  auto-detection. An unknown `--root` crate is a usage error (exit 2).
- **Wired-in** crates are the union of every root's normal-edge dependency
  closure, each listed with the root(s) that pull it in (attribution).
- **Orphans** are members in no root's closure, under their own heading.
- A **dependency tree** per root is included.

The report measures DEPENDENCY-GRAPH wiring (a crate appears in a root's cargo
dependency tree), NOT functional runtime registration; the report states this
distinction explicitly. The gate-run summary section is sourced read-only from
the most recent run in `gate.history.csv` (it notes clearly when no history
exists).

Default output: `<log-dir>/gate.wiring-report.md` (default `.gate/`); override
with `--report-out`. This subcommand and its flags are an ADDITIVE change (new
in 0.2.0): no existing flag, exit-code meaning, or `gate.history.csv` column
changes.

## Exit codes

| Code | Verdict |
|------|---------|
| 0 | CLEAN -- no errors, warnings, or failures |
| 1 | ISSUES -- at least one error/warning/failed test |
| 2 | TIMEOUT -- a step exceeded the watchdog and was terminated, or a usage error |

## Install

```
cargo install cargo-gate
```

Then invoke it as a cargo subcommand from any workspace: `cargo gate`.

## License

MIT OR Apache-2.0.
