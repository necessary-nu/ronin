# The two questions are asked per directory rather than in two passes, so a
# name the makefile offers in an earlier directory beats a file that is really
# there in a later one.
vpath %.te d1/ d2/
all: fail.te
	@printf 'all from %s\n' '$<' > seen
d1/fail.te:
