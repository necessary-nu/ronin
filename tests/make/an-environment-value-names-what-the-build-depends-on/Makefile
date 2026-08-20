# A prerequisite list the environment supplied is makefile text like any other,
# because the binding it arrives in is recursive. The graph the build walks is
# therefore the one the expansion names; a reader that hands the bytes on gets a
# prerequisite literally called `$(addsuffix` and no rule to make it, which is a
# refusal rather than a wrong file — but only because the text happened to start
# with a `$`.
all: $(DEPS)
	@printf 'built=[%s]\n' '$^' > out

a.dep b.dep:
	@printf 'dep\n' > $@
