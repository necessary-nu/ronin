# An implicit pattern rule second-expands each raw prerequisite word alone. A
# trailing backslash cannot quote the blank separating it from the next word.
S = file
.SECONDEXPANSION:
.PHONY: all foo.a\ file
all: foo.pick
foo.a\ file: ; @:
%.pick: %.a\ $$(S) ; @printf wrong > result
