//! Wiring computation: union of per-root dependency closures, per-root
//! attribution, and orphan derivation. Pure over a `WorkspaceFacts` snapshot
//! and the per-root closures, so every acceptance criterion is testable with
//! in-memory fixtures.

use std::collections::{BTreeMap, BTreeSet};

use crate::workspace::{CrateName, WorkspaceFacts};

use super::roots::Roots;

// === Data model =====================================================

/// The computed wiring picture for the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wiring {
    pub roots: Roots,
    /// Each wired-in member mapped to the root(s) whose closure contains it.
    pub wired_in: BTreeMap<CrateName, BTreeSet<CrateName>>,
    /// Members in no root's closure (and not themselves roots).
    pub orphans: BTreeSet<CrateName>,
    /// Per-root closure, restricted to members, retained for the tree section.
    pub closures: BTreeMap<CrateName, BTreeSet<CrateName>>,
    /// Roots whose closure fell back to just the root itself (cargo tree failed).
    pub degraded_roots: BTreeSet<CrateName>,
}

/// Compute the wiring picture.
///
/// `closures` maps each root to its member-restricted dependency closure (as
/// produced by `dependency_closure`, intersected with the member set by the
/// caller). `degraded_roots` names roots whose closure could not be derived and
/// was reduced to `{root}`.
pub fn compute_wiring(
    facts: &WorkspaceFacts,
    roots: Roots,
    closures: BTreeMap<CrateName, BTreeSet<CrateName>>,
    degraded_roots: BTreeSet<CrateName>,
) -> Wiring {
    let members: BTreeSet<CrateName> = facts.members.iter().map(|m| m.name.clone()).collect();

    // Attribution: for each root, record it against every crate in its closure.
    let mut wired_in: BTreeMap<CrateName, BTreeSet<CrateName>> = BTreeMap::new();
    for (root, closure) in &closures {
        for crate_name in closure {
            if members.contains(crate_name) {
                wired_in
                    .entry(crate_name.clone())
                    .or_default()
                    .insert(root.clone());
            }
        }
    }

    // Orphans: members that are in no closure and are not roots themselves.
    let orphans: BTreeSet<CrateName> = members
        .iter()
        .filter(|m| !wired_in.contains_key(*m) && !roots.names.contains(*m))
        .cloned()
        .collect();

    Wiring {
        roots,
        wired_in,
        orphans,
        closures,
        degraded_roots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_cmd::roots::RootProvenance;
    use crate::workspace::MemberFact;

    fn cn(s: &str) -> CrateName {
        CrateName(s.to_string())
    }

    fn member(name: &str) -> MemberFact {
        MemberFact {
            name: cn(name),
            produces_bin: false,
            depends_on: vec![],
        }
    }

    fn facts(names: &[&str]) -> WorkspaceFacts {
        WorkspaceFacts {
            members: names.iter().map(|n| member(n)).collect(),
        }
    }

    fn roots(names: &[&str]) -> Roots {
        Roots {
            names: names.iter().map(|n| cn(n)).collect(),
            provenance: RootProvenance::AutoDetected,
        }
    }

    fn closure(members: &[&str]) -> BTreeSet<CrateName> {
        members.iter().map(|n| cn(n)).collect()
    }

    // The AC-4 model: app -> {app, lib-a}; ffmdx -> {ffmdx, lib-a, lib-b}.
    fn ac4() -> (
        WorkspaceFacts,
        Roots,
        BTreeMap<CrateName, BTreeSet<CrateName>>,
    ) {
        let f = facts(&["app", "ffmdx", "lib-a", "lib-b"]);
        let r = roots(&["app", "ffmdx"]);
        let mut c = BTreeMap::new();
        c.insert(cn("app"), closure(&["app", "lib-a"]));
        c.insert(cn("ffmdx"), closure(&["ffmdx", "lib-a", "lib-b"]));
        (f, r, c)
    }

    // Validates: Requirement 3.2 -- wired-in is the union of per-root closures.
    #[test]
    fn union_of_closures_is_the_wired_in_set() {
        let (f, r, c) = ac4();
        let w = compute_wiring(&f, r, c, BTreeSet::new());
        let wired: BTreeSet<CrateName> = w.wired_in.keys().cloned().collect();
        assert_eq!(wired, closure(&["app", "ffmdx", "lib-a", "lib-b"]));
    }

    // Validates: Requirement 4.1, 4.2 -- orphans are members in no closure and
    // exclude roots.
    #[test]
    fn orphans_are_members_in_no_closure_excluding_roots() {
        let f = facts(&["app", "ffmdx", "lib-a", "lib-b", "orphan-x"]);
        let r = roots(&["app", "ffmdx"]);
        let mut c = BTreeMap::new();
        c.insert(cn("app"), closure(&["app", "lib-a"]));
        c.insert(cn("ffmdx"), closure(&["ffmdx", "lib-a", "lib-b"]));
        let w = compute_wiring(&f, r, c, BTreeSet::new());
        assert_eq!(w.orphans, closure(&["orphan-x"]));
    }

    // Validates: Requirement 5.1, 5.2 -- attribution lists all attributing roots.
    #[test]
    fn attribution_lists_all_roots_pulling_a_crate() {
        let (f, r, c) = ac4();
        let w = compute_wiring(&f, r, c, BTreeSet::new());
        assert_eq!(w.wired_in[&cn("lib-a")], closure(&["app", "ffmdx"]));
        assert_eq!(w.wired_in[&cn("lib-b")], closure(&["ffmdx"]));
    }

    // Validates: Requirement 4.4 -- two runs produce identical sorted output.
    #[test]
    fn two_runs_produce_identical_sorted_output() {
        let (f1, r1, c1) = ac4();
        let (f2, r2, c2) = ac4();
        let w1 = compute_wiring(&f1, r1, c1, BTreeSet::new());
        let w2 = compute_wiring(&f2, r2, c2, BTreeSet::new());
        assert_eq!(w1, w2);
        // BTree iteration order is sorted and stable.
        let keys1: Vec<&CrateName> = w1.wired_in.keys().collect();
        let keys2: Vec<&CrateName> = w2.wired_in.keys().collect();
        assert_eq!(keys1, keys2);
    }
}
