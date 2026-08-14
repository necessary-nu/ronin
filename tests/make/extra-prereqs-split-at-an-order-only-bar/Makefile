.EXTRA_PREREQS = norm | tick
all: | oo
	@printf "bar=[$|] caret=[$^]\n" > out
norm oo tick: FOO = bar
norm oo tick:
	@touch $@
