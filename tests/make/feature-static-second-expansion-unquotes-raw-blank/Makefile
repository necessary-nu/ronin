# A static pattern rule follows the explicit-rule path: it expands the whole
# retained list, then lets a trailing backslash quote the next raw-word blank.
S = file
.SECONDEXPANSION:
.PHONY: all foo.a\ file
all: foo.pick
foo.a\ file: ; @:
foo.pick: %.pick: %.a\ $$(S) ; @printf '<%s> <%s>\n' '$@' '$<' > result
