# The other direction: a `-` below the first line is script text, not a flag,
# so the recipe is not forgiven and the failing last line decides.
.ONESHELL:
all: out
out:
	@touch m1
	-touch m2
	false
