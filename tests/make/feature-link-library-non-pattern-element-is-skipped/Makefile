# An element with no `%` in it has no wildcard to put the name into. GNU Make
# says so and passes it over rather than taking it literally, so the search
# goes on to the next element — which is why `mtest_foo.a` sitting right there
# is not the answer.
.LIBPATTERNS = mtest_foo.a lib%.zz
all: -lfoo ; @printf '%s\n' 'from $<' > out
