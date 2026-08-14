# The deferred path splits the directory the same way, by substituting the file
# part of the stem and putting the directory in front of what the expansion
# yields. `$$*` still reads the whole stem, so the two can be told apart.
.SECONDEXPANSION:
all: lib/bye.x

%.x: $$(EMPTY)6% $$*.d ; @printf '%s\n' '$^' > log; touch $@
