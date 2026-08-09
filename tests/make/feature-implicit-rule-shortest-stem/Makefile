# When two pattern rules match, the one leaving the shorter stem wins, whatever
# order they were read in.
%.x: ; @printf '%s\n' plain > $@
%-mt.x: ; @printf '%s\n' suffixed > $@

all: foo.x foo-mt.x
