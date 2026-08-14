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
    // from 73 when the oracle stopped being whatever `make` on PATH resolved to
    // and became upstream 4.4.1, built from the release tarball and named by
    // its path. Neither tool changed. The corpus reads the name it is handed:
    // three makefiles skip themselves with `$(error test skipped)` when
    // `$(MAKE)` is exactly `make`, and seven scripts print a canned expectation
    // *instead of* running the tool when that name starts with `make`. A Make
    // named by its path runs all ten cases for the first time.
    // `err_export_override.mk#default`, `err_override_export.mk#default` and
    // `wildcard_cache.mk#test` became byte-identical — what had been recorded
    // against them was GNU Make's skip diagnostic against kati's real run — and
    // the seven `kati-extension` scripts kept differing, now against a GNU Make
    // that ran rather than against the corpus's opinion of what it would say.
    //
    // Before that it moved
    // from 74 when `value` learned to read an automatic variable back: the base
    // forms answer with the name they were set to, and the `D` and `F` forms
    // with the `dir`/`notdir` expression GNU Make defined them from, rather than
    // the whole function refusing. `value_at.mk#test` became byte-identical and
    // left the recorded corpus-TODO class.
    //
    // Before that it moved
    // from 76 when an assignment made from inside a `foreach` or `call` binding
    // started landing in the global scope the binding shadows rather than being
    // refused or applied in place: `autovar_assign.mk#default` and
    // `param.mk#test` became byte-identical and left the recorded corpus-TODO
    // class. They are the two cases the vendored corpus wrote the defect down
    // in, each headed by the corpus's own fix-me annotation.
    //
    // Before that it moved
    // from 77 when the export directives were completed: `export_export.mk#test`
    // — `export=PASS` followed by a bare `export export` — became byte-identical
    // and left the recorded corpus-TODO class.
    //
    // Before that it moved
    // from 78 when a recipe's continuation line began losing the recipe prefix
    // it carries, as GNU Make drops it before the line reaches the shell:
    // `multiline_recipe.mk#test6` became byte-identical and left the recorded
    // corpus-TODO class.
    //
    // Before that it moved
    // from 85 when `+=` stopped writing a separator onto a variable that is
    // defined but empty: `cond_syntax.mk#test`, `ifeq_without_parens.mk#test`,
    // `target_specific_var_append.mk#default`, `var_append.mk#test` and the
    // three `makefile_list.mk` cases became byte-identical, and took the
    // `append-to-empty` family with them, being the only cases that named it.
    //
    // Before that it moved
    // from 86 when each ordinary double-colon record became its own action,
    // tested against the prerequisites that record declared:
    // `multi_explicit_output_patterns_double_colon.mk#test` became
    // byte-identical and left the recorded corpus-TODO class.
    //
    // Before that it moved
    // from 88 when rule file-name scanning adopted GNU Make's escaped-blank
    // semantics: `colon_ws_in_file.mk#test` and `colon_ws_in_target.mk#test`
    // became byte-identical and left the recorded corpus-TODO class.
    //
    // Before that it moved
    // from 89 when expanded `=` stopped turning a prerequisite into a
    // target-specific assignment: `equal_in_target.mk#test` became
    // byte-identical and left the recorded corpus-TODO class.
    //
    // Before that it moved from 90 when a missing `include` stopped being
    // reported as a read that failed and started being reported the way GNU
    // Make reports it, as a
    // makefile there is no rule to make: `err_include.mk` became byte-identical
    // and took the `io-error-text` family with it, being the only case that
    // named it.
    //
    // Before that it moved from 93 when `vpath` and `VPATH` were implemented:
    // the corpus has one makefile for the directive, and all three of its cases
    // now match GNU Make where before they were recorded as differing.
    //
    // Before that it moved from 159, when the fatal diagnostics gained Make's
    // name and its `Stop.` and every abandoned build gained Make's exit status:
    // 66 cases became byte-identical, all of them from the defect class.
    assert_eq!(cases.len(), 70);
    assert_eq!(by_class["defect"], 34);
    assert_eq!(by_class["recorded"], 4);
    assert_eq!(by_class["extension"], 31);
    assert_eq!(by_class["artefact"], 1);
}
