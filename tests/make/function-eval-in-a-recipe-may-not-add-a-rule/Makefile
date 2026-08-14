# The graph is compiled before any recipe runs, so a rule recorded from inside
# one could never be part of it.
all: first
	@printf '%s\n' 'ran all' > all-out
first:
	@printf '%s\n' 'ran first' > first-out
	$(eval later: ; @true)
