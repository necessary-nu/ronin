# The same shape under -n. GNU Make turns the switch off across the makefile
# update, so helper, the child and gen.mk are all made for real and only the
# goal is described.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk: helper
	@printf 'GENERATED := from-generated\n' > $@

helper:
	@printf 'h\n' > helper
	@$(MAKE) -C sub marker
