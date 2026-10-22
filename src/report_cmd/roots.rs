//! Root detection and override resolution.
//!
//! Roots are the crates whose dependency closures define the "wired-in" set.
//! They default to auto-detection (every binary-producing member plus every
//! top-of-tree library) and can be replaced by an explicit `--root` list. All
//! logic here is pure over a `WorkspaceFacts` snapshot so it is unit-testable
//! without spawning cargo.

use std::collections::BTreeSet;
use std::fmt;

use crate::workspace::{CrateName, WorkspaceFacts};

// === Types ==========================================================

/// Whether the root set was derived automatically or supplied by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootProvenance {
    AutoDetected,
    UserSpecified,
}

/// The resolved set of root crates and how it was obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub names: BTreeSet<CrateName>,
    pub provenance: RootProvenance,
}

/// Errors from resolving roots.
#[derive(Debug, PartialEq, Eq)]
pub enum RootError {
    /// A user-specified `--root` crate is not a workspace member.
    UnknownRoot(String),
}

impl fmt::Display for RootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RootError::UnknownRoot(name) => {
                write!(f, "unknown root crate '{name}': not a workspace member")
            }
        }
    }
}

impl std::error::Error for RootError {}

// === Resolution =====================================================

/// Resolve the root set from the workspace facts and any `--root` overrides.
///
/// When `overrides` is empty the roots are auto-detected; otherwise the
/// overrides replace auto-detection (each is validated against the member set).
///
/// # Errors
/// Returns [`RootError::UnknownRoot`] when an override names a crate that is not
/// a workspace member.
pub fn resolve_roots(facts: &WorkspaceFacts, overrides: &[String]) -> Result<Roots, RootError> {
    if overrides.is_empty() {
        Ok(Roots {
            names: auto_detect(facts),
            provenance: RootProvenance::AutoDetected,
        })
    } else {
        let members: BTreeSet<&str> = facts.members.iter().map(|m| m.name.as_str()).collect();
        let mut names = BTreeSet::new();
        for raw in overrides {
            if !members.contains(raw.as_str()) {
                return Err(RootError::UnknownRoot(raw.clone()));
            }
            names.insert(CrateName(raw.clone()));
        }
        Ok(Roots {
            names,
            provenance: RootProvenance::UserSpecified,
        })
    }
}

/// Auto-detect roots: every binary-producing member, plus every library member
/// that no other member depends on (a top-of-tree library).
fn auto_detect(facts: &WorkspaceFacts) -> BTreeSet<CrateName> {
    let member_set: BTreeSet<&str> = facts.members.iter().map(|m| m.name.as_str()).collect();

    // Names that some OTHER member depends on (restricted to members).
    let mut depended_on: BTreeSet<&str> = BTreeSet::new();
    for m in &facts.members {
        for dep in &m.depends_on {
            if member_set.contains(dep.as_str()) {
                depended_on.insert(dep.as_str());
            }
        }
    }

    let mut roots = BTreeSet::new();
    for m in &facts.members {
        let is_top_of_tree = !depended_on.contains(m.name.as_str());
        if m.produces_bin || is_top_of_tree {
            roots.insert(m.name.clone());
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::MemberFact;

    fn member(name: &str, bin: bool, deps: &[&str]) -> MemberFact {
        MemberFact {
            name: CrateName(name.to_string()),
            produces_bin: bin,
            depends_on: deps.iter().map(|d| CrateName(d.to_string())).collect(),
        }
    }

    fn cn(s: &str) -> CrateName {
        CrateName(s.to_string())
    }

    // Validates: Requirement 2.2, 2.3 -- auto-detect = binaries plus top-of-tree
    // libraries, excluding mid-tree libraries that another member depends on.
    #[test]
    fn auto_detect_returns_binaries_and_top_libs_excluding_mid_libs() {
        let facts = WorkspaceFacts {
            members: vec![
                member("app", true, &["lib-mid"]),
                member("lib-top", false, &[]),
                member("lib-mid", false, &[]),
            ],
        };
        let roots = resolve_roots(&facts, &[]).expect("resolve");
        assert_eq!(roots.provenance, RootProvenance::AutoDetected);
        let expected: BTreeSet<CrateName> = [cn("app"), cn("lib-top")].into_iter().collect();
        assert_eq!(roots.names, expected);
        assert!(!roots.names.contains(&cn("lib-mid")));
    }

    // Validates: Requirement 2.4 -- an explicit --root list replaces detection.
    #[test]
    fn explicit_root_replaces_auto_detection() {
        let facts = WorkspaceFacts {
            members: vec![
                member("app", true, &["lib-mid"]),
                member("lib-top", false, &[]),
                member("lib-mid", false, &[]),
            ],
        };
        let roots = resolve_roots(&facts, &["app".to_string()]).expect("resolve");
        assert_eq!(roots.provenance, RootProvenance::UserSpecified);
        let expected: BTreeSet<CrateName> = [cn("app")].into_iter().collect();
        assert_eq!(roots.names, expected);
    }

    // Validates: Requirement 2.5 -- an unknown --root crate is an error.
    #[test]
    fn unknown_root_crate_yields_error() {
        let facts = WorkspaceFacts {
            members: vec![member("app", true, &[])],
        };
        let err = resolve_roots(&facts, &["does-not-exist".to_string()]).unwrap_err();
        assert_eq!(err, RootError::UnknownRoot("does-not-exist".to_string()));
    }

    // Validates: Requirement 2.7 -- an empty workspace yields empty roots.
    #[test]
    fn empty_workspace_yields_empty_roots() {
        let facts = WorkspaceFacts { members: vec![] };
        let roots = resolve_roots(&facts, &[]).expect("resolve");
        assert!(roots.names.is_empty());
        assert_eq!(roots.provenance, RootProvenance::AutoDetected);
    }

    // Validates: Requirement 2.7 -- a workspace of only interdependent libraries
    // with a single top still yields that top as a root (never empty).
    #[test]
    fn interdependent_libs_still_select_top_of_tree() {
        let facts = WorkspaceFacts {
            members: vec![
                member("lib-top", false, &["lib-mid"]),
                member("lib-mid", false, &["lib-leaf"]),
                member("lib-leaf", false, &[]),
            ],
        };
        let roots = resolve_roots(&facts, &[]).expect("resolve");
        let expected: BTreeSet<CrateName> = [cn("lib-top")].into_iter().collect();
        assert_eq!(roots.names, expected);
    }
}
