# The single-suffix test is guarded by the suffix not beginning with a dot, so
# a declared `.foo` does not reject a target named `.foo`. Reachable only
# through a name carrying a slash, since a bare `.foo` is rejected for its
# leading dot a line earlier.
.SUFFIXES:
.SUFFIXES: .foo
sub/.foo: ; @printf '%s\n' 'dotfoo ran' > out
zed:      ; @printf '%s\n' 'zed ran'    > out
