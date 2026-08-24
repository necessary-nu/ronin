# A deferred prerequisite is expanded in the TARGET's variable scope, not in
# the ambient one. GNU Make installs the file's variable set around the second
# expansion — `initialize_file_variables` then `set_file_variables` in front of
# `variable_expand_for_file`, in read.c for an explicit target and in
# implicit.c for a pattern rule's candidate — so a target-specific binding and
# every pattern-specific binding whose pattern matches the name are both in
# scope.
#
# Read in the ambient scope instead, `$$a` and `$$b` come back empty and the
# prerequisites they named are not in the graph at all: a build that does not
# happen, rather than a value that comes out wrong.
.SECONDEXPANSION:

all: foo.x bar.x baz.zz ; @printf 'all\n' > all.out

# The explicit rule, reading one target-specific and one pattern-specific
# binding.
foo.x: $$a $$b ; @printf '%s\n' '$@ <- $^' > foo.out
foo.x: a := from-target
%.x: b := from-pattern

# The static pattern rule, which reads the same two scopes.
bar.x: %.x: $$c $$b ; @printf '%s\n' '$@ <- $^' > bar.out
bar.x: c := from-static

# The implicit rule, whose candidate is expanded during the search rather than
# after it. `$$*` is the stem the candidate matched with, so `$$($$*_d)` names
# `baz_d` — a target-specific binding reached through the stem itself.
%.zz: $$($$*_d) $$e ; @printf '%s\n' '$@ <- $^' > baz.out
baz.zz: baz_d := from-implicit
%.zz: e := from-implicit-pattern

from-target from-pattern from-static from-implicit from-implicit-pattern: ; @:
