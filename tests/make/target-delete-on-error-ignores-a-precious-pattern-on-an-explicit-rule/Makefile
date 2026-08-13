# A .PRECIOUS pattern protects what an implicit rule whose target pattern is
# written exactly that way made, and nothing else. This target is spelled out
# by a rule of its own, so no pattern ever spoke for it and it goes.
.DELETE_ON_ERROR:
.PRECIOUS: %.bar

all: foo.bar

foo.bar: ; @printf '%s\n' partial > $@; exit 1
