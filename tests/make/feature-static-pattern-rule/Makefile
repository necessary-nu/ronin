# A static pattern rule applies a pattern to a listed set of targets only, so
# the stem comes from each named target rather than from a search.
files = foo.out bar.out

all: $(files)

$(files): %.out: %.in ; @printf '%s from %s\n' '$@' '$<' > $@
