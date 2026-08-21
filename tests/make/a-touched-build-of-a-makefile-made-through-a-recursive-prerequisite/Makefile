# The same shape under -t. The makefile update is not pretended, so helper, the
# child and gen.mk are made rather than dated, and only the goal is touched.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk: helper
	@printf 'GENERATED := from-generated\n' > $@

helper:
	@printf 'h\n' > helper
	@$(MAKE) -C sub marker
