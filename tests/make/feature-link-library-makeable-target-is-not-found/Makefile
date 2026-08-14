# The same law with nothing on disk to soften it: a pattern that names a target
# the makefile could make is not a match at all, so the reference resolves to
# nothing and the run is refused under the name as written.
.LIBPATTERNS = made_%.a
all: -lfoo ; @printf '%s\n' 'from $<' > out
made_foo.a: ; @touch $@
