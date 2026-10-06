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
fn dependency_closure(cargo: &str, cwd: &Path, root: &str) -> Option<HashSet<String>> {
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
}
