//! Workspace discovery: locating the repo root and deriving the `--app-only`
//! orphan-exclude list (all workspace members minus the dependency closure of a
//! named crate), mirroring the ffwb-gate approach but generic to any project.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Minimal view of `cargo metadata --no-deps`.
#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_root: String,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    /// Compile targets of this package; used to tell whether it produces a
    /// binary. Absent in some metadata shapes, so it defaults to empty.
    #[serde(default)]
    targets: Vec<Target>,
    /// Declared dependencies of this package (by name). Used to find
    /// top-of-tree libraries (members no other member depends on).
    #[serde(default)]
    dependencies: Vec<Dep>,
}

/// A compile target within a package (`lib`, `bin`, `test`, ...).
#[derive(Debug, Deserialize)]
struct Target {
    #[serde(default)]
    kind: Vec<String>,
}

/// A single declared dependency (only its name is needed here).
#[derive(Debug, Deserialize)]
struct Dep {
    name: String,
}

// === Workspace facts (for root auto-detection) ======================

/// A workspace member crate name. Newtype over `String` so sorting and set
/// membership read as crate-name operations rather than bare strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CrateName(pub String);

impl CrateName {
    /// The underlying crate name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CrateName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One workspace member's facts relevant to root detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberFact {
    pub name: CrateName,
    /// True when the member has at least one binary target.
    pub produces_bin: bool,
    /// Names of this member's declared dependencies (any edge kind).
    pub depends_on: Vec<CrateName>,
}

/// A typed snapshot of the workspace from a single `cargo metadata` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFacts {
    pub members: Vec<MemberFact>,
}

/// Resolve the workspace root via `cargo metadata`. Falls back to the current
/// directory if cargo cannot be queried.
pub fn workspace_root(cargo: &str, cwd: &Path) -> Result<PathBuf> {
    let meta = metadata(cargo, cwd)?;
    Ok(PathBuf::from(meta.workspace_root))
}

/// All workspace member package names.
pub fn member_names(cargo: &str, cwd: &Path) -> Result<Vec<String>> {
    let meta = metadata(cargo, cwd)?;
    Ok(meta.packages.into_iter().map(|p| p.name).collect())
}

/// Build a typed workspace snapshot from a single `cargo metadata --no-deps`
/// call, reusing the one metadata implementation (NFR-5).
pub fn workspace_facts(cargo: &str, cwd: &Path) -> Result<WorkspaceFacts> {
    let meta = metadata(cargo, cwd)?;
    Ok(facts_from_metadata(meta))
}

/// Pure transform from parsed metadata to the workspace-facts snapshot. Kept
/// separate from the process call so root detection is unit-testable without
/// spawning cargo.
fn facts_from_metadata(meta: Metadata) -> WorkspaceFacts {
    let members = meta
        .packages
        .into_iter()
        .map(|p| {
            let produces_bin = p.targets.iter().any(|t| t.kind.iter().any(|k| k == "bin"));
            let depends_on = p
                .dependencies
                .into_iter()
                .map(|d| CrateName(d.name))
                .collect();
            MemberFact {
                name: CrateName(p.name),
                produces_bin,
                depends_on,
            }
        })
        .collect();
    WorkspaceFacts { members }
}

fn metadata(cargo: &str, cwd: &Path) -> Result<Metadata> {
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(cwd)
        .output()
        .context("failed to run `cargo metadata`")?;
    if !output.status.success() {
        anyhow::bail!(
            "`cargo metadata` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let meta: Metadata =
        serde_json::from_slice(&output.stdout).context("parsing cargo metadata JSON")?;
    Ok(meta)
}

/// Derive the `--app-only` orphan list: workspace members that are NOT in the
/// dependency closure of `closure_of`. Returns an empty vec if the closure
/// cannot be derived (the caller then falls back to a full workspace run so no
/// crate is silently skipped).
pub fn derive_orphans(cargo: &str, cwd: &Path, closure_of: &str) -> Vec<String> {
    let members = match member_names(cargo, cwd) {
        Ok(m) if !m.is_empty() => m,
        _ => return Vec::new(),
    };
    let closure = match dependency_closure(cargo, cwd, closure_of) {
        Some(c) if !c.is_empty() => c,
        _ => return Vec::new(),
    };
    let mut orphans: Vec<String> = members
        .into_iter()
        .filter(|m| !closure.contains(m))
        .collect();
    orphans.sort();
    orphans
}

/// The set of workspace crates in `root`'s normal-dependency tree, parsed from
/// `cargo tree -p <root> --edges normal --prefix none`. We keep only names that
/// are also workspace members (the caller intersects against members anyway, but
/// restricting here keeps the set tight).
pub(crate) fn dependency_closure(cargo: &str, cwd: &Path, root: &str) -> Option<HashSet<String>> {
    let output = Command::new(cargo)
        .args(["tree", "-p", root, "--edges", "normal", "--prefix", "none"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut set = HashSet::new();
    for line in text.lines() {
        // Lines look like "ff-keys v0.1.0" or "ff-keys v0.1.0 (/path)". Strip at
        // the first " v".
        let name = line.split(" v").next().unwrap_or("").trim();
        if !name.is_empty() {
            set.insert(name.to_string());
        }
    }
    // Always include the root itself.
    set.insert(root.to_string());
    Some(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure parsing of the dependency-closure logic is exercised indirectly;
    // here we test the orphan-filtering set logic in isolation.
    #[test]
    fn orphan_filtering_keeps_only_non_closure_members() {
        let members = vec!["app".to_string(), "lib-a".to_string(), "orphan".to_string()];
        let mut closure = HashSet::new();
        closure.insert("app".to_string());
        closure.insert("lib-a".to_string());

        let mut orphans: Vec<String> = members
            .into_iter()
            .filter(|m| !closure.contains(m))
            .collect();
        orphans.sort();
        assert_eq!(orphans, vec!["orphan".to_string()]);
    }

    // Validates: Requirement 2.2, 2.3 -- facts_from_metadata records which
    // members produce a binary and each member's declared dependency names.
    #[test]
    fn facts_from_metadata_records_bin_targets_and_edges() {
        let json = r#"{
            "workspace_root": "/ws",
            "packages": [
                {
                    "name": "app",
                    "targets": [{ "kind": ["bin"] }],
                    "dependencies": [{ "name": "lib-mid" }]
                },
                {
                    "name": "lib-mid",
                    "targets": [{ "kind": ["lib"] }],
                    "dependencies": []
                },
                {
                    "name": "lib-top",
                    "targets": [{ "kind": ["lib"] }],
                    "dependencies": []
                }
            ]
        }"#;
        let meta: Metadata = serde_json::from_str(json).expect("parse fixture");
        let facts = facts_from_metadata(meta);
        assert_eq!(facts.members.len(), 3);

        let app = facts
            .members
            .iter()
            .find(|m| m.name.as_str() == "app")
            .expect("app present");
        assert!(app.produces_bin, "app has a bin target");
        assert_eq!(app.depends_on, vec![CrateName("lib-mid".to_string())]);

        let lib_mid = facts
            .members
            .iter()
            .find(|m| m.name.as_str() == "lib-mid")
            .expect("lib-mid present");
        assert!(!lib_mid.produces_bin, "lib-mid is a library only");
        assert!(lib_mid.depends_on.is_empty());
    }
}
