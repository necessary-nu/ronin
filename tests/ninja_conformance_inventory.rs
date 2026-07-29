use std::collections::BTreeSet;

const INVENTORY: &str = include_str!("ninja_suite_inventory.tsv");
const PINNED_REVISION: &str = "b51a1e37c2fb89bbefa600bd155e1ce13983f09d";

// [spec:samurai:req:compat.upstream-conformance/test]
#[test]
fn pinned_ninja_inventory_is_complete_and_has_no_silent_exclusions() {
    assert!(INVENTORY
        .lines()
        .next()
        .is_some_and(|line| line.ends_with(PINNED_REVISION)));

    let mut suites = BTreeSet::new();
    let mut overrides = BTreeSet::new();
    let mut test_count = 0_usize;
    for (line_number, line) in INVENTORY.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            5,
            "inventory line {} must have five tab-separated fields",
            line_number + 1
        );
        assert!(
            !fields[4].is_empty(),
            "inventory line {} needs evidence or a reason",
            line_number + 1
        );
        match fields[0] {
            "suite" => {
                assert!(suites.insert(fields[1]), "duplicate suite {}", fields[1]);
                test_count += fields[2]
                    .parse::<usize>()
                    .expect("suite count must be numeric");
                assert!(
                    matches!(fields[3], "mapped" | "rust-native"),
                    "suite {} has an unclassified disposition",
                    fields[1]
                );
            }
            "test" => {
                assert!(
                    overrides.insert(fields[1]),
                    "duplicate test override {}",
                    fields[1]
                );
                assert_eq!(fields[2], "-");
                assert!(
                    matches!(
                        fields[3],
                        "mapped" | "rust-native" | "platform-inapplicable"
                    ),
                    "test {} has an unclassified disposition",
                    fields[1]
                );
            }
            kind => panic!("unknown inventory kind {kind}"),
        }
    }
    assert_eq!(suites.len(), 33);
    assert_eq!(test_count, 425);
    assert_eq!(
        overrides,
        BTreeSet::from(["PathEscaping.SensibleWin32PathsAreNotNeedlesslyEscaped"])
    );
    assert!(!INVENTORY.contains("pending"));
    assert!(!INVENTORY.contains("unclassified"));
}
