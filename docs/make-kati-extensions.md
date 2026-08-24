# The kati extensions, and their removal

Ronin's Make front end is a GNU Make 4.4.1 frontend. Its evaluator is the
vendored, ported kati — which is not only a Make clone: it added surface of its
own, for Android's build. That surface arrived here with the port. Nobody asked
for it.

**Operator ruling, Brendan, 2026-08-24**, verbatim: *"fuck kati extensions"* —
clarified in a follow-up as: **remove the kati-only extensions from the
product**. Ronin implements GNU Make; the kati extensions are inherited surface
nobody asked for.

**This reverses a standing campaign rule.** Until that ruling the rule was *"kati
extensions are deliberate product surface — never delete one under cover of a
compatibility fix"*. That rule was an **agent-era convention, not his ruling**:
no operator ever said the extensions were product, and the convention existed to
stop a dispatch quietly deleting surface while claiming to fix conformance. His
ruling supersedes it. The convention's *purpose* survives in a narrower form,
and it is what makes the reversal safe rather than a licence: **an extension may
still not be deleted as a side effect of a conformance fix.** It is deleted
deliberately, by name, with its corpus cases audited — which is what this
document is for.

## What counts as an extension

Makefile text — or a switch — that reaches kati-only behaviour GNU Make 4.4.1
does not have. Three things are deliberately **not** on the list:

- **Implementation markers a makefile cannot name.** `KATI_NEW_INPUTS`,
  `KATI_NEW_INPUTS_D`, `KATI_NEW_INPUTS_F` and `KATI_SETTLED_<n>` are shell
  variable names the compiler invents for its own lowering of `$?` and of a
  prerequisite spelling that is not settled yet. `tests/make_regressions.rs`
  already asserts, in six cases, that they never reach a reader's eyes. They are
  internal plumbing carrying a prefix, not surface.
- **Gaps.** kati has no `$(guile ...)` and no `load`/`-load`. Those are the
  opposite of extensions and are not touched here.
- **Warnings and switches the front end never accepts.** See *Reachability*.

## Reachability, established once

Ronin's Make front end drives kati's **shared** evaluator — `src/make.rs`
imports `kati::evaluate::{Evaluated, evaluate}` and calls it — so everything
implemented in `eval.rs`, `func.rs`, `dep.rs`, `var.rs` and `parser.rs` is
reachable from `ronin make`. Measured, not assumed:

```
$(info KATI=[$(KATI)])
$(info varloc=[$(KATI_variable_location FOO)])
$(info fsep=[$(KATI_foreach_sep v,-,a b c,x$(v))])
```

| | Ronin `make` (before) | GNU Make 4.4.1 |
| --- | --- | --- |
| `KATI` | `ckati` | *(empty)* |
| `KATI_variable_location` | `Makefile:2` | *(empty)* |
| `KATI_foreach_sep` | `xa-xb-xc` | *(empty)* |

What is **not** reachable is anything gated on a `Flags` field only
`kati::flags::Flags::from_args` sets: Ronin builds `Flags` by hand and closes
with `..Flags::default()` (`src/make/cli.rs`), and refuses unknown long options
outright. So **no `--kati_*`-class switch is product surface for Ronin's make** —
`--detect_depfiles`, `--regen`, `--use_find_emulator`, `--gen_all_targets`,
`--warn_*`/`--werror_*`, `--writable`, `--ninja_dir` and the rest are reachable
only from the `rkati` binary, which is the evaluator with nothing else on top and
exists so the conformance gate can measure evaluation against GNU Make.

## The inventory

### Builtin functions — twelve, all removed

Every one was in `FUNC_INFO` in `kati/src-rs/func.rs`, and reachable from
`ronin make`.

| Function | What it did |
| --- | --- |
| `$(KATI_deprecated_var V[,msg])` | every later read of `V` warns |
| `$(KATI_obsolete_var V[,msg])` | every later read of `V` is a fatal error, and `V` leaves `.VARIABLES` |
| `$(KATI_deprecate_export msg)` | every later `export` warns |
| `$(KATI_obsolete_export msg)` | every later `export` is a fatal error |
| `$(KATI_profile_makefile f…)` | marks a makefile interesting in `--kati_stats` output |
| `$(KATI_variable_location V…)` | expands to `file:line` per name |
| `$(KATI_extra_file_deps f…)` | errors if `f` is missing; adds it to the regeneration stamp's dependency set |
| `$(KATI_shell_no_rerun cmd)` | `$(shell)` whose answer a regeneration replays instead of re-running |
| `$(KATI_foreach_sep V,SEP,LIST,TEXT)` | `foreach` with an explicit separator |
| `$(KATI_file_no_rerun OP[,text])` | `$(file …)` a regeneration replays instead of repeating |
| `$(KATI_visibility_prefix V,prefix…)` | restricts which source files may reference `V` |
| `$(KATI_debug_var V…)` | prints each name's value and flavour to stdout |

`KATI_debug_var` had no counterpart in the vendored C++ table at all — it was
added by the Rust port.

The machinery behind them went with them, because nothing else set it:
`Variable::deprecated` / `obsolete` / `visibility_prefix` and the `used()` and
`check_current_referencing_file()` checks every variable read performed;
`Evaluator::export_allowed`, `ExportAllowed` and `check_export_allowed()`;
`Evaluator::profiled_files` and `stats::mark_interesting`;
`MakefileCache::add_extra_file_dep`. The file cache's own set of files a read
depended on and could not open stays — an `include` that would not open is still
a dependency, and it is not an extension — under the name `unread`.

`$(shell)` and `$(file …)` keep their `rerun` distinction internally, because the
regeneration stamp still records what a read asked the ground; what is gone is
the makefile's ability to ask for the other side of it.

### Special variables and targets — nine

| Name | Shape | Status |
| --- | --- | --- |
| `.KATI_READONLY` | global and target-specific variable | **removed** |
| `.KATI_ALLOW_RULES` | global variable | **removed** |
| `.KATI_SYMBOLS` | readable variable, a filtered `.VARIABLES` | **removed** |
| `.KATI_RESTAT` | special target | *pending* |
| `.KATI_DEPFILE` | target-specific variable → Ninja `depfile` | *pending* |
| `.KATI_IMPLICIT_OUTPUTS` | target-specific variable → Ninja implicit outputs | *pending* |
| `.KATI_NINJA_POOL` | target-specific variable → Ninja `pool` | *pending* |
| `.KATI_TAGS` | target-specific variable, opaque metadata | *pending* |
| `.KATI_VALIDATIONS` | target-specific variable → Ninja validations | *pending* |

`.KATI_VALIDATIONS` is the one that is already unreachable from `ronin make`
without being removed: it is gated on `--use_ninja_validations`, which Ronin's
front end never sets, so a makefile that names it is refused rather than obeyed.

### Assignment operator — `$=`

`FOO :=$= bar` marked the binding readonly on assignment; a later write to it was
a fatal `cannot assign to readonly variable`. It was not a distinct operator in
the grammar but a `$=` prefix on the right-hand side of any of them, stripped by
`parser.rs` into `AssignStmt::is_final`. It is the per-variable spelling of
`.KATI_READONLY` and went with it. **Removed.**

Every other operator in the table is GNU's: `=`, `:=`, `::=`, `:::=`, `+=`,
`?=`, `!=`.

### The `KATI` variable

kati's bootstrap makefile sets `KATI?=ckati`, and `ifdef KATI` is how the
vendored corpus asks which tool is running it. It is not a name GNU Make defines.
*Pending.*

### Directives and syntax

None. The directive table is exactly GNU's, and the builtin-variable catalogue
is GNU's; the only kati name in the special-target list is `.KATI_RESTAT`.

## What was removed, and what it cost

### The twelve functions (2026-08-24)

**Removed outright.** No internal dependent: nothing in `src/`, in the build
engine, or in the regeneration stamp needed a makefile to be able to call one.

**Corpus.** Twenty-seven cases of the vendored kati suite went with them,
because a case that only tests a removed extension is not a case about Make:

- deleted: `deprecated_export.mk`, `deprecated_var.mk`,
  `err_deprecated_var_already_deprecated.mk`,
  `err_deprecated_var_already_obsolete.mk`, `err_obsolete_export.mk`,
  `err_obsolete_var.mk`, `err_obsolete_var_already_deprecated.mk`,
  `err_obsolete_var_already_obsolete.mk`, `err_obsolete_var_assign.mk`,
  `err_obsolete_var_msg.mk`, `err_obsolete_var_varref.mk`,
  `err_obsolete_var_varsubst.mk`, `variable_location.mk`,
  `var_visibility_prefix_conflict.mk`,
  `var_visibility_prefix_implicit_define.mk`,
  `var_visibility_prefix_invalid_file_{one,two,four}.mk`,
  `var_visibility_prefix_invalid_prefix_{one,two,three,four}.mk`,
  `ninja_file_no_rerun_func.sh`, `ninja_shell_no_rerun.sh`,
  `ninja_shell_no_rerun_error_in_rule.sh`, `ninja_regen_extra_file_deps.sh`,
  `ninja_regen_extra_file_deps_error_on_missing_file.sh`
- rewritten, because they were mixed: `foreach.mk` loses its `ifdef KATI`
  branch and stays the GNU `$(foreach)` test it always also was;
  `ninja_regen.sh` loses the two `$(KATI_*_var)` lines from the makefile it
  rewrites mid-script and stays the regeneration test it is.

**Conformance movement**, audited case by case. 387 runs → 365; normalised
336 identical / 51 differing → 324 / 41; raw 286 identical → 275. Every one of
the ten rows that left the inventory was class `extension`, family
`kati-extension`: `deprecated_export.mk#test`, `deprecated_var.mk#test` and the
eight `var_visibility_prefix_*` cases. The other seventeen deleted cases carried
no row at all — their `ifdef KATI` fallback made GNU Make's side agree by
construction, which is exactly the shape of a case that tests the extension and
nothing else. **No case that tests Make semantics moved.**

**Unit tests.** One went: `func::tests::test_foreach_sep_restores_unbound_variable_when_body_fails`,
which asserted `KATI_foreach_sep` restores a binding its body failed inside. The
`$(foreach)` case beside it — the same property, on the GNU function — stays and
is the one that gates the behaviour.

### The readonly family (2026-08-24)

`.KATI_READONLY`, the `$=` final-assignment operator that is its per-variable
spelling, `.KATI_ALLOW_RULES` and `.KATI_SYMBOLS` — removed together, because
they are one thing in the code: three well-known symbols and a readonly bit that
nothing else set.

**Removed outright.** `Variable::readonly` is gone, and with it every check that
read it: `GlobalVars::assign`'s and `ScopedVars::assign`'s refusal, `undefine`'s,
and `+=`'s. All four were reachable only from these two spellings — GNU Make has
no readonly notion for variables at all, which is what `.KATI_READONLY` was added
to supply. Three call chains lost an out-parameter they had carried only to
report a refusal that can no longer happen.

`.KATI_SYMBOLS` took a whole mechanism with it. `Evaluable::is_func` existed to
answer *"would reading this value call something?"* without reading it, because
`.KATI_SYMBOLS` is `.VARIABLES` minus the bindings whose value looks like a
function. Its own comment says it is a heuristic. `.VARIABLES` never asked, so
the trait method, both implementations and the `all` flag that selected between
the two name lists are gone.

`.KATI_ALLOW_RULES` took `RulesAllowed` and the per-rule check the evaluator made
against it — a makefile could ask that recording a rule from here on be a warning
or an error, which GNU Make has no way to say.

**Corpus.** Nine cases deleted: `readonly_global.sh`,
`readonly_global_missing.sh`, `readonly_rule.sh`, `readonly_rule_missing.sh`,
`final_global.sh`, `final_rule.sh`, `final_rule2.sh`, `allow_rules.sh`,
`shellstatus_readonly.mk` — the last of which asserted `.SHELLSTATUS` cannot be
written through a computed name, which is the readonly rule under another name.
`variables.mk` was rewritten rather than deleted: it tests `.VARIABLES` and
`.KATI_SYMBOLS` in one file, and the `.VARIABLES` half is Make.

**Conformance movement.** 365 runs → 356; normalised 324 identical / 41 differing
→ 324 / 32; raw 275 identical, unmoved. All nine rows that left were class
`extension`, family `kati-extension`, and they are exactly the nine deleted
cases. No case that tests Make semantics moved, and nothing that was identical
stopped being identical.
