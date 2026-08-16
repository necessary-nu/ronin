# `-R` withholds the tool catalogue and nothing else. The path forms of the
# automatic variables are part of the language rather than part of the
# toolchain, so `define_automatic_variables` runs whatever `-R` says and
# `$(origin @D)` still answers `automatic` while `$(origin CC)` has become
# `undefined`.
$(file >out,@D origin=[$(origin @D)] value=[$(value @D)])
$(file >>out,%F origin=[$(origin %F)] value=[$(value %F)])
$(file >>out,CC origin=[$(origin CC)] value=[$(value CC)])

.PHONY: all
all: ; @:
