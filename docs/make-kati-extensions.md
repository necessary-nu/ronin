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
| `.KATI_RESTAT` | special target | **removed** |
| `.KATI_DEPFILE` | target-specific variable → Ninja `depfile` | **removed** |
| `.KATI_IMPLICIT_OUTPUTS` | target-specific variable → Ninja implicit outputs | **removed** |
| `.KATI_NINJA_POOL` | target-specific variable → Ninja `pool` | **removed** |
| `.KATI_TAGS` | target-specific variable, opaque metadata | **removed** |
| `.KATI_VALIDATIONS` | target-specific variable → Ninja validations | **removed** |

`.KATI_VALIDATIONS` was already unreachable from `ronin make` before it was
removed: it was gated on `--use_ninja_validations`, which Ronin's front end never
sets, so a makefile that named it was refused rather than obeyed.

### Assignment operator — `$=`

`FOO :=$= bar` marked the binding readonly on assignment; a later write to it was
a fatal `cannot assign to readonly variable`. It was not a distinct operator in
the grammar but a `$=` prefix on the right-hand side of any of them, stripped by
`parser.rs` into `AssignStmt::is_final`. It is the per-variable spelling of
`.KATI_READONLY` and went with it. **Removed.**

Every other operator in the table is GNU's: `=`, `:=`, `::=`, `:::=`, `+=`,
`?=`, `!=`.

### The `KATI` variable

kati's bootstrap makefile set `KATI?=ckati`, and `ifdef KATI` is how the vendored
corpus asked which tool was reading it. It is not a name GNU Make defines.
**Removed.**

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

### The `KATI` variable (2026-08-24)

**Removed outright** — one line out of the bootstrap makefile kati reads before
the real one.

**Corpus, and this one is the interesting case: nothing was deleted.** Three
cases used `ifdef KATI`, and all three gate a GNU Make feature rather than an
extension, so all three had the gate deleted and now run their real branch on
both tools:

- `file_func.sh` used `ifdef KATI` *or* `$(filter 4.2%,$(MAKE_VERSION))` to
  decide whether `$(file …)` exists at all. Both tools are 4.4.1 and both have
  it, so the gate could only ever send GNU Make down the "pretend the answer"
  path. Un-gated, the two agree byte for byte.
- `ninja_regen_filefunc_read.sh` used it for the same reason, around a
  `$(file <…)` the regeneration test needs.
- `shellstatus_in_rule.mk` used it to choose between running `.SHELLSTATUS`
  inside a rule and printing a canned sentence saying kati cannot. **The
  sentence was stale.** Un-gated, kati reads `.SHELLSTATUS` inside a rule
  exactly as GNU Make does, and the two agree byte for byte.

**Conformance movement.** 356 runs, unchanged — no case was deleted. Normalised
324 identical / 32 differing → 326 / 30; raw 275 identical → 276. Two rows left
because the two tools now agree: `file_func.sh#script`, which was the last case
of class `artefact` and took the `corpus-version-gate` family with it, and
`shellstatus_in_rule.mk#test`. Two cases that tested a version gate and a
self-report are now two cases that test Make.

### The six Ninja-edge target variables (2026-08-24)

`.KATI_DEPFILE`, `.KATI_RESTAT`, `.KATI_IMPLICIT_OUTPUTS`, `.KATI_NINJA_POOL`,
`.KATI_TAGS` and `.KATI_VALIDATIONS`. Each named a property of the Ninja edge a
target compiles to, and GNU Make has a spelling for none of them.

**What was removed, and what was kept, differ per name — this is the family where
the distinction matters.**

- **Removed outright, mechanism and all**: `.KATI_TAGS` (opaque metadata nothing
  in the build reads), `.KATI_VALIDATIONS` (nothing else in Make mode produces a
  validation, so the `--use_ninja_validations` flag went with it), and
  `.KATI_RESTAT` (Make-target freshness is what Make *is* — every Make edge is
  re-observed after its recipe runs — so a narrower per-target request was
  surface on top of a property the graph already had).
- **Spelling removed, mechanism kept because a GNU feature needs it**:
  `.KATI_IMPLICIT_OUTPUTS`. A grouped target (`out out.stamp &: in`) is GNU's own
  way to say one recipe makes several files, and it fills the same
  `implicit_outputs` list. What went is the `.KATI_IMPLICIT_OUTPUTS` variable and
  the `RuleMerger` bookkeeping that existed only for it — including a
  parent-merger chain that let an implicit output redirect which rule a target
  picked, which grouped targets do not use.
- **Spelling removed, mechanism kept because a switch still reaches it**:
  `.KATI_DEPFILE` and `.KATI_NINJA_POOL`. `--detect_depfiles` still finds a
  depfile by reading the assembled script, and `--default_pool` /
  `--remote_num_jobs` still name a pool; both are `rkati` switches, not product
  surface (see *Reachability*). `.NOTPARALLEL`'s serialising pool is GNU's and is
  untouched.

**What `.KATI_DEPFILE` cost to remove, said plainly.** It was the only makefile
text in Make mode that declared a depfile, so a Make-compiled graph now reaches
Ninja's dependency log by no route a Makefile can ask for. A GNU makefile says
this with `-include`, which Ronin compiles: the recipe writes `main.o.d`, the
next run reads it as ordinary makefile text, and editing a discovered header
rebuilds the object one run later than a runtime depfile would — **which is
exactly what GNU Make does**. `tests/make_state.rs` was rewritten to that idiom
and its `state_preserves_discovered_dependencies` case is now
`a_discovered_dependency_reaches_the_next_build`, asserting the same user-visible
property by the route GNU takes. Both logs are still placed, named and formatted
as Ninja's, which is what `[spec:ronin:req:make.state-outside-the-tree+2]` asks.

**It also dissolves divergence #4** of `make-oracle-divergences.md`: a
`.KATI_DEPFILE` recipe's `$(file …)` was performed where the recipe was read
rather than where it ran, because the edge had to declare the depfile it would
read and a deferred rule had no path to. With no makefile-reachable depfile there
is no such recipe, and Make mode now defers every recipe kind it ever deferred.

**Gates deleted because they tested the extension rather than the product**:
`tests/shell.rs::a_depfile_recipe_launches_per_line` and
`::a_depfile_recipe_is_read_where_built` (the latter was divergence #4's own
gate), and the `make_port` fixture
`feature-vpath-a-depfile-recipe-spells-the-found-name`.
`src/make/equivalence.rs::a_validation_agrees` went with `.KATI_VALIDATIONS`;
`::the_per_edge_bindings_agree` was rewritten around a grouped target, and a new
`::a_detected_depfile_agrees` keeps the `depfile` binding under gate by the route
that still reaches it.

**Corpus.** Seven cases deleted: `ninja_implicit_outputs.sh`,
`ninja_implicit_output_var.sh`, `ninja_implicit_dependent.sh`, `ninja_pool.sh`,
`ninja_validations.sh`, `phony_looks_real.sh` and `real_no_cmds.sh`. The last two
are the interesting ones: their subject is a kati *warning switch*, but they
build the shape it complains about out of `.KATI_IMPLICIT_OUTPUTS` and cannot be
written without it.

**Conformance movement.** 356 runs → **354**; normalised 326 identical / 30
differing → 326 / 28; raw 276 identical, unmoved. Seven cases were deleted and
the run count falls by two, because five of the seven are `ninja_*` scripts the
make oracle never enumerates. The two rows that left are exactly
`phony_looks_real.sh#script` and `real_no_cmds.sh#script`.

*(The commit that landed this family said 349 rather than 354 in its message. The
count above is the measured one; the error was arithmetic, subtracting all seven
deleted cases from the run total when five of them were never in it.)*

## What was NOT removed, and why

**kati-only command-line switches.** `--detect_depfiles`, `--regen` and the
regeneration stamp, `--use_find_emulator`, `--gen_all_targets`,
`--empty_ninja_file`, `--warn_*`/`--werror_*`, `--writable`, `--top_level_phony`,
`--default_pool`, `--ninja_dir`, `--kati_stats` and the rest. **Ronin's `make`
accepts none of them** — it builds its `Flags` by hand and refuses any long
option it does not know — so they are not product surface. They reach only
`rkati`, which is the evaluator with nothing else on top and exists so the
conformance gate can measure evaluation against GNU Make. Removing them would be
removing the measurement apparatus, not the product's surface. This is a
judgement about the boundary rather than about the switches, and it is written
down here so it can be overruled.

**`KATI_NEW_INPUTS`, `KATI_NEW_INPUTS_D`, `KATI_NEW_INPUTS_F`,
`KATI_SETTLED_<n>`.** Compiler-invented shell variable names, not makefile
surface. `tests/make_regressions.rs` asserts in six cases that they never reach a
reader. They keep the prefix because renaming them would be churn with no
behaviour behind it.

## The movement in one place

| | runs | normalised identical | normalised differing | raw identical | `kati-extension` rows |
| --- | --- | --- | --- | --- | --- |
| before | 387 | 336 | 51 | 286 | 31 |
| after the twelve functions | 365 | 324 | 41 | 275 | 20 |
| after the readonly family | 356 | 324 | 32 | 275 | 11 |
| after the `KATI` variable | 356 | 326 | 30 | 275 | 11 |
| after the six edge variables | **354** | **326** | **28** | **276** | **8** |

Forty-three corpus cases were deleted and every one is named above. Twenty-three
inventory rows left, and every one was class `extension` except
`file_func.sh#script` — the last `artefact` case, which left because the two
tools started **agreeing**. Nothing that was byte-identical stopped being so.

The eight rows still in the `kati-extension` family name no makefile syntax at
all: `empty_ninja_file.sh`, `implicit_pattern_rule_warn.sh`,
`real_no_cmds_or_deps.sh`, `real_to_phony.sh`, `suffix_rule_warn.sh`,
`top_level_phony.sh`, `werror_overriding_commands.sh` and `writable.sh` each
drive a kati-only **switch**, which is the surface this removal deliberately left
alone. `werror_find_emulator.sh` is a ninth `extension`-class case under the
`find-emulator` family for the same reason.
