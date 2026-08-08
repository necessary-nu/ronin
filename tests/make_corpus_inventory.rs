use std::collections::{BTreeMap, BTreeSet};

const INVENTORY: &str = include_str!("make_corpus_inventory.tsv");
const CLASSES: [&str; 4] = ["defect", "recorded", "extension", "artefact"];

/// The inventory is the classification, so it is checked without running the
/// corpus: an entry that names no family, or a family that explains nothing,
/// would let a difference through without anyone deciding what it is.
#[test]
fn every_recorded_make_difference_is_classified_and_every_family_is_used() {
    let mut families = BTreeMap::new();
    let mut cases = BTreeSet::new();
    let mut by_class: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, line) in INVENTORY.lines().enumerate() {
        let number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["family", name, reason] => {
                assert!(!reason.is_empty(), "family {name} has no reason");
                assert!(
                    families.insert(*name, 0_usize).is_none(),
                    "duplicate family {name}"
                );
            }
            ["case", id, class, names, digest] => {
                assert!(
                    CLASSES.contains(class),
                    "line {number}: case {id} is in unknown class {class}"
                );
                assert!(cases.insert(*id), "duplicate case {id}");
                assert_eq!(
                    digest.len(),
                    16,
                    "line {number}: case {id} has no full digest"
                );
                assert!(
                    u64::from_str_radix(digest, 16).is_ok(),
                    "line {number}: case {id} has a non-hexadecimal digest"
                );
                for name in names.split('+') {
                    *families
                        .get_mut(name)
                        .unwrap_or_else(|| panic!("case {id} names undeclared family {name}")) += 1;
                }
                *by_class.entry(*class).or_default() += 1;
            }
            _ => panic!("line {number} is not a family or case record"),
        }
    }

    for (name, used) in &families {
        assert!(*used > 0, "family {name} explains nothing");
    }
    assert!(!INVENTORY.contains("unclassified"));
    assert!(!INVENTORY.contains("pending"));
    // The count is pinned so that a difference appearing or disappearing is a
    // decision somebody made rather than a number that drifted. It last moved
    // from 93 when `vpath` and `VPATH` were implemented: the corpus has one
    // makefile for the directive, and all three of its cases now match GNU Make
    // where before they were recorded as differing.
    //
    // Before that it moved from 159, when the fatal diagnostics gained Make's
    // name and its `Stop.` and every abandoned build gained Make's exit status:
    // 66 cases became byte-identical, all of them from the defect class.
    assert_eq!(cases.len(), 90);
    assert_eq!(by_class["defect"], 44);
    assert_eq!(by_class["recorded"], 14);
    assert_eq!(by_class["extension"], 31);
    assert_eq!(by_class["artefact"], 1);
}
