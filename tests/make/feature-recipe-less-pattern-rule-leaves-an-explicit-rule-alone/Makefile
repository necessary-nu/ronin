# The same answer where the target has a rule of its own: a rule the search
# never collects contributes no prerequisite to it either.
foo.gen: ; @printf 'from [%s]\n' '$^' > $@
%.gen: %.src

all: foo.gen
