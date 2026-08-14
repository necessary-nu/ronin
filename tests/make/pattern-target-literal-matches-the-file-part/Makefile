# The literal a pattern begins with has to match the file part of the name, not
# the whole of it: `l` starts `lib/` but does not start `bye.x`, so that rule
# does not apply, while `b%.x` does and leaves a stem of `lib/ye`.
all: lib/bye.x

l%.x: dep ; @printf 'lpattern stem=%s\n' '$*' > log; touch $@
b%.x: dep ; @printf 'bpattern stem=%s\n' '$*' > log; touch $@

dep: ; @touch dep
