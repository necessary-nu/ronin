# The same rule written terminal, which is what `::` means on a pattern rule.
# A terminal match-anything rule is allowed to make an intermediate, so `foo`
# is made from `foo.y`, used, and removed again.
all: foo.out

%.out: % ; @cat $< > $@
%:: %.y ; @cat $< > $@
