# Where GNU Make puts an `--eval` fragment in MAKEFLAGS, and how. After the
# switch group and before the `--` that ends the switches, with a `$` doubled
# so reading the variable does not expand what the fragment says. MFLAGS is the
# same options without it: GNU Make defines MFLAGS from the switch string and
# only then appends the eval flags, so a fragment is in one variable and not
# the other.
all: ; @printf 'MAKEFLAGS=[%s]\nMFLAGS=[%s]\nX=[%s]\n' '$(MAKEFLAGS)' '$(MFLAGS)' '$(X)' > out
