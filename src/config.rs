//! Command-line argument parsing and run configuration.
//!
//! `cargo-gate` is a cargo subcommand, so argv[1] is the literal string
//! `gate` when invoked as `cargo gate`. We strip it before parsing.

use std::path::PathBuf;

use clap::Parser;

/// Mode of the run, recorded in history so ETAs compare like with like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Full,
    Fast,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Full => "FULL",
            Mode::Fast => "FAST",
        }
    }
}

/// Target of the optional post-test build phase (`--build`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildTarget {
    /// Build the active scope (so `--build` honours `--crate` / `--app-only`).
    Scope,
    /// Build a single named binary/crate (`-p <name>`).
    Bin(String),
}

/// The test-step package scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Full `--workspace` -- the canonical completion gate.
    Workspace,
    /// `--workspace --exclude <orphans>`: only the dependency closure of a binary.
    AppOnly { closure_of: String },
    /// `-p <name>`: a single crate.
    Crate { name: String },
}

impl Scope {
    /// Short label used as the history `scope` column (compared for ETA).
    pub fn label(&self) -> String {
        match self {
            Scope::Workspace => "workspace".to_string(),
            Scope::AppOnly { .. } => "app-only".to_string(),
            Scope::Crate { name } => format!("crate:{name}"),
        }
    }

    /// Human-readable description for console banners.
    pub fn describe(&self) -> String {
        match self {
            Scope::Workspace => "workspace (full completion gate)".to_string(),
            Scope::AppOnly { closure_of } => {
                format!("app-only (dependency closure of {closure_of})")
            }
            Scope::Crate { name } => format!("crate: {name}"),
        }
    }
}

/// Raw CLI surface.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-gate",
    bin_name = "cargo gate",
    about = "A unified verification gate: fmt + clippy + nextest with one per-phase summary.",
    version
)]
pub struct Cli {
    /// Developer inner-loop mode: sets PROPTEST_CASES=32 for a quick signal.
    /// NOT the full gate.
    #[arg(long)]
    pub fast: bool,

    /// Test only the dependency closure of the crate named by --closure-of,
    /// excluding workspace orphan crates. Requires --closure-of.
    #[arg(long, requires = "closure_of")]
    pub app_only: bool,

    /// The binary/library crate whose dependency closure defines --app-only.
    #[arg(long, value_name = "CRATE")]
    pub closure_of: Option<String>,

    /// Gate a single named crate only (`cargo nextest run -p <name>`).
    #[arg(long = "crate", value_name = "CRATE")]
    pub crate_name: Option<String>,

    /// Directory for log artifacts. Defaults to ./.gate relative to the workspace.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,

    /// Per-step watchdog timeout in seconds. A step running longer is assumed
    /// hung and terminated.
    #[arg(long, value_name = "SECS", default_value_t = 1800)]
    pub step_timeout: u64,

    /// Opt in to a build phase AFTER a clean gate, so you are left with a
    /// runnable binary (fmt/clippy/test alone do NOT produce the app binary).
    /// Bare `--build` builds the active scope; `--build <bin>` builds `-p <bin>`.
    /// Skipped automatically if the gate is not clean. Off by default so the
    /// default run stays a pure, side-effect-free verification gate.
    #[arg(long, value_name = "BIN", num_args = 0..=1)]
    pub build: Option<Option<String>>,

    /// Build with the release profile (composes with --build). A release build
    /// uses a different profile and shares NO artifacts with the test run, so it
    /// is substantially slower than a debug relink.
    #[arg(long)]
    pub release: bool,
}

/// Fully resolved configuration for a run.
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub scope: Scope,
    pub log_dir: PathBuf,
    pub step_timeout_secs: u64,
    /// The optional post-test build phase; `None` means no build phase (default).
    pub build: Option<BuildTarget>,
    /// Build with the release profile when the build phase runs.
    pub release: bool,
}

impl Config {
    /// Parse argv, stripping the leading `gate` cargo-subcommand token.
    pub fn from_args() -> Result<Self, clap::Error> {
        let raw: Vec<String> = std::env::args().collect();
        let filtered: Vec<String> = strip_subcommand_token(raw);
        let cli = Cli::try_parse_from(filtered)?;
        Ok(Self::from_cli(cli))
    }

    fn from_cli(cli: Cli) -> Self {
        let mode = if cli.fast { Mode::Fast } else { Mode::Full };
        let scope = if let Some(name) = cli.crate_name {
            Scope::Crate { name }
        } else if cli.app_only {
            // clap `requires` guarantees closure_of is present here.
            Scope::AppOnly {
                closure_of: cli.closure_of.expect("clap enforces --closure-of"),
            }
        } else {
            Scope::Workspace
        };
        let log_dir = cli.log_dir.unwrap_or_else(|| PathBuf::from(".gate"));
        let build = match cli.build {
            None => None,
            Some(None) => Some(BuildTarget::Scope),
            Some(Some(name)) => Some(BuildTarget::Bin(name)),
        };
        Config {
            mode,
            scope,
            log_dir,
            step_timeout_secs: cli.step_timeout,
            build,
            release: cli.release,
        }
    }
}

/// When invoked as `cargo gate ...`, argv is `["cargo-gate", "gate", ...]`.
/// Drop the redundant `gate` token so clap sees a normal argument list. When
/// invoked directly (`cargo-gate ...`) there is no token to drop.
fn strip_subcommand_token(mut args: Vec<String>) -> Vec<String> {
    if args.len() >= 2 && args[1] == "gate" {
        args.remove(1);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_gate_subcommand_token_when_present() {
        let args = vec![
            "cargo-gate".to_string(),
            "gate".to_string(),
            "--fast".to_string(),
        ];
        let out = strip_subcommand_token(args);
        assert_eq!(out, vec!["cargo-gate".to_string(), "--fast".to_string()]);
    }

    #[test]
    fn leaves_args_untouched_when_invoked_directly() {
        let args = vec!["cargo-gate".to_string(), "--fast".to_string()];
        let out = strip_subcommand_token(args.clone());
        assert_eq!(out, args);
    }

    #[test]
    fn default_scope_is_full_workspace() {
        let cli = Cli::try_parse_from(["cargo-gate"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.scope, Scope::Workspace);
        assert_eq!(cfg.mode, Mode::Full);
    }

    #[test]
    fn fast_flag_selects_fast_mode() {
        let cli = Cli::try_parse_from(["cargo-gate", "--fast"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.mode, Mode::Fast);
    }

    #[test]
    fn crate_flag_selects_single_crate_scope() {
        let cli = Cli::try_parse_from(["cargo-gate", "--crate", "ff-keys"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(
            cfg.scope,
            Scope::Crate {
                name: "ff-keys".to_string()
            }
        );
    }

    #[test]
    fn app_only_requires_closure_of() {
        let err = Cli::try_parse_from(["cargo-gate", "--app-only"]);
        assert!(err.is_err(), "--app-only without --closure-of must error");
    }

    #[test]
    fn app_only_with_closure_resolves_scope() {
        let cli = Cli::try_parse_from(["cargo-gate", "--app-only", "--closure-of", "my-app"])
            .expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(
            cfg.scope,
            Scope::AppOnly {
                closure_of: "my-app".to_string()
            }
        );
    }

    #[test]
    fn build_flag_absent_is_none() {
        // Validates: Requirement 15.1 -- without --build, no build phase.
        let cli = Cli::try_parse_from(["cargo-gate"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.build, None);
        assert!(!cfg.release);
    }

    #[test]
    fn build_flag_bare_targets_active_scope() {
        // Validates: Requirement 15.2 -- bare --build builds the active scope.
        let cli = Cli::try_parse_from(["cargo-gate", "--build"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.build, Some(BuildTarget::Scope));
    }

    #[test]
    fn build_flag_named_targets_single_bin() {
        // Validates: Requirement 15.3 -- --build <bin> builds -p <bin>.
        let cli = Cli::try_parse_from(["cargo-gate", "--build", "mybin"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.build, Some(BuildTarget::Bin("mybin".to_string())));
    }

    #[test]
    fn release_flag_sets_release() {
        // Validates: Requirement 15.6 -- --release selects the release profile.
        let on = Config::from_cli(Cli::try_parse_from(["cargo-gate", "--release"]).expect("parse"));
        assert!(on.release);
        let off = Config::from_cli(Cli::try_parse_from(["cargo-gate"]).expect("parse"));
        assert!(!off.release);
    }

    #[test]
    fn build_and_release_compose() {
        // Validates: Requirement 15.6 -- --release composes with --build.
        let cli = Cli::try_parse_from(["cargo-gate", "--build", "--release"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.build, Some(BuildTarget::Scope));
        assert!(cfg.release);
    }

    #[test]
    fn build_does_not_disturb_scope() {
        // Bare --build must not consume the following flag as its value.
        let cli = Cli::try_parse_from(["cargo-gate", "--build", "--crate", "x"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(
            cfg.scope,
            Scope::Crate {
                name: "x".to_string()
            }
        );
        assert_eq!(cfg.build, Some(BuildTarget::Scope));
    }
}
