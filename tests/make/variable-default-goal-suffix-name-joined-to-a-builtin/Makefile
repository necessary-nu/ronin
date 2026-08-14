# The two-suffix test has no leading-dot guard, which is what makes the
# built-in list matter: `foo.c` is the declared `foo` followed by the built-in
# `.c`, and is passed over.
.SUFFIXES: foo
foo.c: ; @printf '%s\n' 'foo.c ran' > out
zed:   ; @printf '%s\n' 'zed ran'   > out
