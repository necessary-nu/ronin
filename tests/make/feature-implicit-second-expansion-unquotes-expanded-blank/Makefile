# An implicit pattern's one raw prerequisite word can expand to an escaped
# blank, which is unquoted only after that word's second expansion.
S = a\ file
.SECONDEXPANSION:
.PHONY: all foo.a\ file
all: foo.pick
foo.a\ file: ; @:
%.pick: %.$$(S) ; @printf '<%s> <%s>\n' '$@' '$<' > result
