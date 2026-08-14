# The same makefile under `-r`, where there is no built-in list for `foo` to
# join to. `foo.c` is nothing but a name and becomes the default goal — which
# is what makes the previous case evidence about the built-in list rather than
# about the target.
.SUFFIXES: foo
foo.c: ; @printf '%s\n' 'foo.c ran' > out
zed:   ; @printf '%s\n' 'zed ran'   > out
