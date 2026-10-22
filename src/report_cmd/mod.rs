//! The `cargo gate report` subcommand: a metadata-and-tree analysis that writes
//! a Markdown report of workspace dependency-graph wiring (which members are
//! reachable from the project's roots and which are orphans), with per-root
//! attribution and a summary of the most recent gate run.
//!
//! The pure analysis (root detection, union closure, orphan derivation,
//! rendering) lives in `roots`, `wiring`, and `render`; this module does the IO:
//! it calls the shared `workspace` machinery, reads the history CSV, and writes
//! the report.

pub mod render;
pub mod roots;
pub mod summary;
pub mod wiring;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::ReportConfig;
use crate::workspace::{self, CrateName};

use roots::{resolve_roots, RootError};

// === Constants ======================================================

/// Default report file name, written under the active log dir.
pub const DEFAULT_REPORT_NAME: &str = "gate.wiring-report.md";

/// Default log dir, matching the gate's default.
const DEFAULT_LOG_DIR: &str = ".gate";

/// History CSV name under the log dir (matches the gate's writer).
const HISTORY_NAME: &str = "gate.history.csv";

// === Errors =========================================================

/// Errors from the report subcommand. All map to exit 2.
#[derive(Debug)]
pub enum ReportError {
    /// A `--root` crate is not a workspace member.
    UnknownRoot(RootError),

    /// `cargo metadata` could not be run or parsed.
    Metadata(anyhow::Error),

    /// The report file could not be written.
    WriteOutput {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReportError::UnknownRoot(e) => write!(f, "{e}"),
            ReportError::Metadata(e) => write!(f, "workspace analysis failed: {e}"),
            ReportError::WriteOutput { path, source } => {
                write!(f, "failed to write report to {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReportError::UnknownRoot(e) => Some(e),
            ReportError::Metadata(e) => Some(e.as_ref()),
            ReportError::WriteOutput { source, .. } => Some(source),
        }
    }
}

impl From<RootError> for ReportError {
    fn from(e: RootError) -> Self {
        ReportError::UnknownRoot(e)
    }
}

// === Path resolution ================================================

/// Resolve the output path for the report.
///
/// When `out` is set it is used verbatim; otherwise the report goes to
/// `<log_dir>/gate.wiring-report.md`.
pub fn resolve_report_path(out: &Option<PathBuf>, log_dir: &Path) -> PathBuf {
    match out {
        Some(p) => p.clone(),
        None => log_dir.join(DEFAULT_REPORT_NAME),
    }
}

// === Entry point ====================================================

/// Run the report subcommand, returning a process exit code (0 success,
/// 2 usage/fatal).
pub fn run_report(config: &ReportConfig) -> i32 {
    match run_report_inner(config) {
        Ok(path) => {
            println!("cargo-gate: wrote wiring report to {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("cargo-gate: {e}");
            2
        }
    }
}

fn run_report_inner(config: &ReportConfig) -> Result<PathBuf, ReportError> {
    let cargo = cargo_bin();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let log_dir = config
        .log_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG_DIR));

    // 1. Workspace facts (single metadata call). A metadata failure is fatal.
    let facts = workspace::workspace_facts(&cargo, &cwd).map_err(ReportError::Metadata)?;

    // 2. Resolve roots (auto-detect or explicit override).
    let roots = resolve_roots(&facts, &config.roots)?;
    let provenance = roots.provenance;

    // 3. Per-root closures. A per-root cargo tree failure degrades to {root}.
    let member_set: BTreeSet<CrateName> = facts.members.iter().map(|m| m.name.clone()).collect();
    let mut closures: BTreeMap<CrateName, BTreeSet<CrateName>> = BTreeMap::new();
    let mut degraded_roots: BTreeSet<CrateName> = BTreeSet::new();
    for root in &roots.names {
        let closure = match workspace::dependency_closure(&cargo, &cwd, root.as_str()) {
            Some(raw) if !raw.is_empty() => raw
                .into_iter()
                .map(CrateName)
                .filter(|c| member_set.contains(c))
                .collect::<BTreeSet<CrateName>>(),
            _ => {
                degraded_roots.insert(root.clone());
                let mut only_root = BTreeSet::new();
                only_root.insert(root.clone());
                only_root
            }
        };
        closures.insert(root.clone(), closure);
    }

    // 4. Compute wiring.
    let wire = wiring::compute_wiring(&facts, roots, closures, degraded_roots);

    // 5. Latest gate run summary from history (read-only; may be None).
    let history_path = log_dir.join(HISTORY_NAME);
    let run_summary = summary::latest_run_summary(&history_path);

    // 6. Render Markdown.
    let markdown = render::render_report(&wire, &run_summary, &provenance);

    // 7. Write the report (creating parent dirs).
    let path = resolve_report_path(&config.out, &log_dir);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| ReportError::WriteOutput {
                path: path.clone(),
                source,
            })?;
        }
    }
    std::fs::write(&path, markdown).map_err(|source| ReportError::WriteOutput {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// The cargo binary to invoke (honours the `CARGO` env var cargo sets for
/// subcommands, falling back to `cargo`).
fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Validates: Requirement 9.1 / AC-12 -- default resolves to
    // <log-dir>/gate.wiring-report.md.
    #[test]
    fn default_report_path_is_under_log_dir() {
        let log_dir = PathBuf::from(".gate");
        let path = resolve_report_path(&None, &log_dir);
        assert_eq!(path, PathBuf::from(".gate").join("gate.wiring-report.md"));
    }

    // Validates: Requirement 9.2 / AC-13 -- an override is honoured verbatim.
    #[test]
    fn override_report_path_is_honoured() {
        let log_dir = PathBuf::from(".gate");
        let out = Some(PathBuf::from("custom/report.md"));
        let path = resolve_report_path(&out, &log_dir);
        assert_eq!(path, PathBuf::from("custom/report.md"));
    }
}
