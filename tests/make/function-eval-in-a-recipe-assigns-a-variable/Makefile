# An assignment is not a graph change. It happens where the recipe is expanded
# and every later expansion reads it.
all: first second
first:
	$(eval V = from-first)
	@printf '%s\n' 'first sees [$(V)]' > first-out
second: first
	@printf '%s\n' 'second sees [$(V)]' > second-out
