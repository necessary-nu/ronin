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
    // from 30 when the six `.KATI_*` target variables that name Ninja edge
    // properties were removed from the product on the same ruling:
    // `.KATI_DEPFILE`, `.KATI_RESTAT`, `.KATI_IMPLICIT_OUTPUTS`,
    // `.KATI_NINJA_POOL`, `.KATI_TAGS` and `.KATI_VALIDATIONS`. Seven cases went
    // with them and TWO rows left, both class `extension`, family
    // `kati-extension`: `phony_looks_real.sh#script` and `real_no_cmds.sh#script`,
    // which built the shape their warning switches complain about out of
    // `.KATI_IMPLICIT_OUTPUTS` and cannot be written without it. The other five
    // are `ninja_*` scripts the make oracle never enumerates and that carried no
    // rows.
    //
    // Before that it moved
    // from 32 when the `KATI` variable was removed from the product on the same
    // ruling. kati's bootstrap set `KATI?=ckati`, and `ifdef KATI` was how the
    // vendored corpus asked which tool was reading it. Three cases used it and
    // NONE was deleted, because all three gate a GNU Make feature rather than an
    // extension: `file_func.sh` and `ninja_regen_filefunc_read.sh` used it to
    // decide whether `$(file ...)` exists, and `shellstatus_in_rule.mk` used it
    // to choose between running `.SHELLSTATUS` inside a rule and printing a
    // canned sentence saying kati cannot. All three had their gate deleted so
    // the real branch always runs, and TWO ROWS LEFT BECAUSE THE TWO TOOLS NOW
    // AGREE: `file_func.sh#script` (the last `artefact` case, which took the
    // `corpus-version-gate` family with it, being the only case that named it)
    // and `shellstatus_in_rule.mk#test`. The canned sentence was stale: kati
    // reads `.SHELLSTATUS` inside a rule exactly as GNU Make does. Two cases
    // that tested a version gate and a self-report are now two cases that test
    // Make.
    //
    // Before that it moved
    // from 41 when the readonly family was removed from the product on the same
    // ruling — `.KATI_READONLY`, the `$=` final-assignment operator that is its
    // per-variable spelling, `.KATI_ALLOW_RULES` and `.KATI_SYMBOLS`. Nine rows
    // went, all of class `extension` and family `kati-extension`: the four
    // `readonly_*.sh` scripts, the three `final_*.sh` scripts,
    // `allow_rules.sh#script` and `shellstatus_readonly.mk#test`. Every one of
    // the nine cases was deleted with the feature. `variables.mk` was rewritten
    // rather than deleted, keeping its `.VARIABLES` half and losing its
    // `.KATI_SYMBOLS` half, and it carried no row either way.
    //
    // Before that it moved
    // from 51 when the twelve `KATI_*` builtin functions were removed from the
    // product on the operator's ruling — see docs/make-kati-extensions.md.
    // Twenty-seven corpus cases went with them, because a case that only tests
    // a removed extension is not a case about Make. Ten of the twenty-seven
    // carried inventory rows, all of class `extension` and family
    // `kati-extension`: `deprecated_export.mk#test`, `deprecated_var.mk#test`,
    // and the eight `var_visibility_prefix_*` cases. The other seventeen —
    // the `err_deprecated_var_*` and `err_obsolete_*` makefiles,
    // `variable_location.mk`, and the five `ninja_*` scripts the make oracle
    // never enumerates — had no rows to lose, because their `ifdef KATI`
    // fallback made GNU Make's side agree by construction.
    //
    // Before that it moved
    // from 52 when an `ifeq`'s close started being found by counting parens
    // forward rather than by reading the line's last byte:
    // `err_invalid_ifeq5.mk#test` — `ifeq (foo, bar) XXX`, headed by the
    // corpus's own fix-me — became identical after normalisation and left the
    // recorded corpus-TODO class, taking its `evaluation` row with it.
    //
    // Before that it moved
    // from 53 when text after an `endif` stopped ending the read and became the
    // warning GNU Make raises: `warn_extra_trailings.mk#default` — three
    // trailing-text directives in four lines, headed by the corpus's own
    // fix-me — became identical after normalisation and left the recorded
    // corpus-TODO class, taking its `evaluation` and `exit-status` rows with
    // it.
    //
    // Before that it moved
    // from 69 when kati adopted GNU Make 4.4.1's `'x'` quoting in place of the
    // 3.8x `` `x' `` it was written against. Sixteen cases became byte-identical
    // — `call_with_whitespace.mk#test`, `define_with_comments.mk#test`, the
    // eight `err_*` conditional and `word`/`wordlist` cases,
    // `func_backslash.mk#test`, `nothing_to_do.mk#default`,
    // `warn_output_pattern_mismatch.mk#test` and the rest — and took the
    // `quote-style` family with them, being the only cases that named it. The
    // four rows that had carried it beside another family kept the other one:
    // `err_override.mk#test` and `override.mk#test` still say `commands` where
    // 4.4.1 says `recipe`, `file_func.sh#script` still turns on the corpus's
    // `MAKE_VERSION` gate, and `werror_overriding_commands.sh#script` is still
    // a kati extension whose GNU side the case writes by hand.
    //
    // Before that it moved
    // from 70 when a circular dependency stopped being refused and started
    // being dropped, the way GNU Make drops the one edge it was standing on
    // when it noticed and carries on building: `circular_dep.mk#test` became
    // byte-identical and left the defect class.
    //
    // Before that it moved
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
    assert_eq!(cases.len(), 28);
    assert_eq!(by_class["defect"], 17);
    assert_eq!(by_class["recorded"], 2);
    assert_eq!(by_class["extension"], 9);
    assert_eq!(by_class.get("artefact"), None);
}
