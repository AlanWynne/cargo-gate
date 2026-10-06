# Contributing to cargo-gate

Thanks for your interest in improving cargo-gate. It is a small, single-crate
cargo plugin, so the contribution loop is simple.

## Getting started

```
git clone https://github.com/AlanWynne/cargo-gate
cd cargo-gate
cargo build
```

Install your local build as the `cargo gate` subcommand to dogfood it:

```
cargo install --path . --force
```

## Before you open a pull request

Run the same three checks the gate itself runs, and make sure they are clean:

```
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```

Then dogfood the tool on itself (an empty `.gate/gate.review.log` means clean):

```
cargo gate
```

## Conventions

- Write a test first for new behavior or a bug fix; keep tests deterministic.
- Keep output ASCII and cross-platform (the tool runs in varied terminals and CI).
- Prefer parsing structured machine output (`--message-format=json`,
  `nextest list --message-format json`) over scraping human text; keep the regex
  fallbacks for the experimental libtest-json stream.
- Keep source files reasonably small and single-purpose.

## Public contract -- call out breaking changes

These are a stability contract for people who already use the tool. Changing any
of them is a breaking change and should be called out in the PR and reflected in
a version bump:

- the CLI surface (flag names and meanings),
- the exit-code meanings (0 CLEAN, 1 ISSUES, 2 TIMEOUT/usage),
- the `gate.history.csv` column set.

## Larger changes

For anything beyond a small fix, please open an issue first so we can agree on
the approach before you invest in a large PR.

## License

By contributing, you agree that your contributions will be dual licensed under
MIT OR Apache-2.0, matching the project license.
