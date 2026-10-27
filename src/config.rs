//! Command-line argument parsing and run configuration.
//!
//! `cargo-gate` is a cargo subcommand, so argv[1] is the literal string
//! `gate` when invoked as `cargo gate`. We strip it before parsing.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    /// The DEFAULT: let Cargo fall back to the workspace `default-members` set.
    /// Emits NO package selection (no `--workspace`, no `-p`), so Cargo selects
    /// `default-members` (or the whole workspace when the manifest declares no
    /// `default-members`).
    DefaultMembers,
    /// Full `--workspace` (alias `--all`) -- every workspace member.
    Workspace,
    /// `--workspace --exclude <orphans>`: only the dependency closure of a binary.
    AppOnly { closure_of: String },
    /// `-p <name>`: a single crate.
    Crate { name: String },
}

impl Scope {
    /// Short label used as the history `scope` column (compared for ETA). The
    /// DEFAULT scope carries its OWN label value ("default-members") so its
    /// per-scope ETA medians never mix with the full-workspace history. This is
    /// a new label VALUE only; the CSV columns are unchanged (public contract).
    pub fn label(&self) -> String {
        match self {
            Scope::DefaultMembers => "default-members".to_string(),
            Scope::Workspace => "workspace".to_string(),
            Scope::AppOnly { .. } => "app-only".to_string(),
            Scope::Crate { name } => format!("crate:{name}"),
        }
    }

    /// Human-readable description for console banners.
    pub fn describe(&self) -> String {
        match self {
            Scope::DefaultMembers => "default-members (Cargo default set)".to_string(),
            Scope::Workspace => "workspace (all members)".to_string(),
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
    /// Optional subcommand. When absent, the gate runs with the flags below,
    /// exactly as before (the public gate contract is preserved).
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Developer inner-loop mode: sets PROPTEST_CASES=32 for a quick signal.
    /// NOT the full gate.
    #[arg(long)]
    pub fast: bool,

    /// Gate the FULL workspace (every member), the behaviour that used to be the
    /// default. Without this the gate runs over Cargo's `default-members` set.
    /// `--all` is an accepted alias.
    #[arg(long = "workspace", visible_alias = "all")]
    pub workspace: bool,

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

/// The optional subcommands. Only `report` exists; its absence means "run the
/// gate" (the historical default).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write a Markdown report of workspace dependency-graph wiring: which
    /// members are reachable from the project's roots and which are orphans.
    Report(ReportArgs),
}

/// Flags for the `report` subcommand.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Repeatable root override; an explicit list replaces auto-detection.
    #[arg(long = "root", value_name = "CRATE")]
    pub roots: Vec<String>,

    /// Output path override; default <log-dir>/gate.wiring-report.md.
    #[arg(long = "report-out", value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Log dir (shared meaning with the gate; default .gate).
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<PathBuf>,
}

/// Fully resolved configuration for a `report` run.
#[derive(Debug, Clone)]
pub struct ReportConfig {
    /// Explicit root overrides; empty means auto-detect.
    pub roots: Vec<String>,
    /// Output path override; `None` means the default under `log_dir`.
    pub out: Option<PathBuf>,
    /// Log dir; `None` means the default `.gate`.
    pub log_dir: Option<PathBuf>,
}

impl ReportConfig {
    fn from_args(args: ReportArgs) -> Self {
        ReportConfig {
            roots: args.roots,
            out: args.out,
            log_dir: args.log_dir,
        }
    }
}

/// The resolved invocation: either run the gate or write a wiring report.
#[derive(Debug, Clone)]
pub enum Invocation {
    Gate(Config),
    Report(ReportConfig),
}

impl Invocation {
    /// Parse argv, stripping the leading `gate` cargo-subcommand token, and
    /// resolve to either a gate run or a report run.
    pub fn from_args() -> Result<Self, clap::Error> {
        let raw: Vec<String> = std::env::args().collect();
        let filtered: Vec<String> = strip_subcommand_token(raw);
        let cli = Cli::try_parse_from(filtered)?;
        Ok(Self::from_cli(cli))
    }

    fn from_cli(cli: Cli) -> Self {
        match cli.command {
            Some(Command::Report(args)) => Invocation::Report(ReportConfig::from_args(args)),
            None => Invocation::Gate(Config::from_cli(cli)),
        }
    }
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
    fn from_cli(cli: Cli) -> Self {
        let mode = if cli.fast { Mode::Fast } else { Mode::Full };
        // Scope precedence (most specific wins):
        //   --crate > --app-only > --workspace/--all > DEFAULT (default-members).
        let scope = if let Some(name) = cli.crate_name {
            Scope::Crate { name }
        } else if cli.app_only {
            // clap `requires` guarantees closure_of is present here.
            Scope::AppOnly {
                closure_of: cli.closure_of.expect("clap enforces --closure-of"),
            }
        } else if cli.workspace {
            Scope::Workspace
        } else {
            Scope::DefaultMembers
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
    fn default_scope_is_default_members() {
        // Validates: Requirement 16.1 -- bare `cargo gate` scopes to Cargo
        // default-members (no --workspace, no -p), the new default.
        let cli = Cli::try_parse_from(["cargo-gate"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.scope, Scope::DefaultMembers);
        assert_eq!(cfg.mode, Mode::Full);
    }

    #[test]
    fn workspace_flag_selects_full_workspace() {
        // Validates: Requirement 16.2 -- --workspace selects the full member set.
        let cli = Cli::try_parse_from(["cargo-gate", "--workspace"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.scope, Scope::Workspace);
    }

    #[test]
    fn all_alias_selects_full_workspace() {
        // Validates: Requirement 16.2 -- --all is an alias for --workspace.
        let cli = Cli::try_parse_from(["cargo-gate", "--all"]).expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(cfg.scope, Scope::Workspace);
    }

    #[test]
    fn crate_flag_beats_workspace_flag() {
        // Validates: Requirement 16.4 -- precedence --crate > --workspace.
        let cli = Cli::try_parse_from(["cargo-gate", "--workspace", "--crate", "ff-keys"])
            .expect("parse");
        let cfg = Config::from_cli(cli);
        assert_eq!(
            cfg.scope,
            Scope::Crate {
                name: "ff-keys".to_string()
            }
        );
    }

    #[test]
    fn app_only_beats_workspace_flag() {
        // Validates: Requirement 16.4 -- precedence --app-only > --workspace.
        let cli = Cli::try_parse_from([
            "cargo-gate",
            "--workspace",
            "--app-only",
            "--closure-of",
            "my-app",
        ])
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
    fn default_members_and_workspace_labels_differ() {
        // Validates: Requirement 16.3 -- the default scope carries its own
        // history label value so its ETAs do not mix with full-workspace ones.
        assert_eq!(Scope::DefaultMembers.label(), "default-members");
        assert_eq!(Scope::Workspace.label(), "workspace");
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

    // Validates: Requirement 1.2 -- no subcommand still yields a gate run with
    // the gate config unchanged.
    #[test]
    fn no_subcommand_resolves_to_a_gate_run() {
        let cli = Cli::try_parse_from(["cargo-gate", "--fast"]).expect("parse");
        match Invocation::from_cli(cli) {
            Invocation::Gate(cfg) => {
                assert_eq!(cfg.mode, Mode::Fast);
                assert_eq!(cfg.scope, Scope::DefaultMembers);
            }
            Invocation::Report(_) => panic!("expected a gate run"),
        }
    }

    // Validates: Requirement 1.1, 2.4, 9.2 -- the report subcommand carries the
    // roots and the output override.
    #[test]
    fn report_subcommand_carries_roots_and_out_override() {
        let cli = Cli::try_parse_from([
            "cargo-gate",
            "report",
            "--root",
            "a",
            "--root",
            "b",
            "--report-out",
            "x.md",
        ])
        .expect("parse");
        match Invocation::from_cli(cli) {
            Invocation::Report(cfg) => {
                assert_eq!(cfg.roots, vec!["a".to_string(), "b".to_string()]);
                assert_eq!(cfg.out, Some(PathBuf::from("x.md")));
            }
            Invocation::Gate(_) => panic!("expected a report run"),
        }
    }

    // Validates: Requirement 2.1 -- report with no --root carries an empty root
    // list (auto-detect).
    #[test]
    fn report_subcommand_without_root_is_auto_detect() {
        let cli = Cli::try_parse_from(["cargo-gate", "report"]).expect("parse");
        match Invocation::from_cli(cli) {
            Invocation::Report(cfg) => {
                assert!(cfg.roots.is_empty());
                assert_eq!(cfg.out, None);
                assert_eq!(cfg.log_dir, None);
            }
            Invocation::Gate(_) => panic!("expected a report run"),
        }
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
