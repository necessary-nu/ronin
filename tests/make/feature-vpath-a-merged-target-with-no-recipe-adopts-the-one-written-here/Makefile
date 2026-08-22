# The kept object has no recipe of its own, so it takes the searched name's
# instead and says nothing about it. What it does not take is the name: the
# recipe runs for the found path.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

fail.te: ; @printf 'made [%s]\n' '$@' > seen2
d/fail.te:
