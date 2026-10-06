//! `cargo-gate`: a unified verification gate for Cargo projects.
//!
//! Runs `cargo fmt --check`, `cargo clippy`, and `cargo nextest run` (or
//! `cargo test`) in sequence, rendering them as one live progress display, then
//! writes combined/review/timing logs and a per-step history CSV and prints a
//! single pass/fail verdict.
//!
//! Invoke as a cargo subcommand: `cargo gate [--fast] [--crate NAME]
//! [--app-only --closure-of NAME] [--log-dir DIR]`.

mod config;
mod display;
mod gate;
mod history;
mod phase;
mod phases;
mod report;
mod runner;
mod workspace;

use std::process::ExitCode;

use config::Config;

fn main() -> ExitCode {
    let config = match Config::from_args() {
        Ok(c) => c,
        Err(e) => {
            // clap prints help/version/usage to the right stream already.
            e.print().ok();
            return ExitCode::from(2);
        }
    };

    match gate::run(&config) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("cargo-gate: fatal error: {e:#}");
            ExitCode::from(2)
        }
    }
}
