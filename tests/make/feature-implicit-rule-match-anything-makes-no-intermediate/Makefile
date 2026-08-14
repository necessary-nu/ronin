# A match-anything rule that is not terminal may not make an intermediate. The
# search for `foo.out` needs `foo`, and `%: %.y` is the only rule that could
# make it, so the search runs out even though `foo.y` is on disk.
all: foo.out

%.out: % ; @cat $< > $@
%: %.y ; @cat $< > $@
