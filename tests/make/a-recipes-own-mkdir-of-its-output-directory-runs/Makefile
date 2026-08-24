# The line every makefile in the world writes, and the reason it writes it:
# nothing else creates `sub/`. It stands first, it is silent, and it asks for
# exactly the output's own directory — which is the shape a compiler emitting
# for Ninja is entitled to absorb, because Ninja would have made the directory
# before launching the command. Absorbing it here deletes a step of the build.
all: sub/out
sub/out: in
	@mkdir -p $(@D)
	@cp $< $@
