# Deprecating the name is not keeping it as a pattern. `%.o` names one file
# here and makes no other, so `foo.o` has no rule and the build is refused
# before the goal above it is reached.
all: foo.o
	@printf top > out
a %.o: ; @printf 'mixed %s\n' '$@' > $@
