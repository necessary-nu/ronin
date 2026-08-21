# The same shape under -B. GNU Make turns always_make_flag off across the
# makefile update of a restarted read (main.c, always_make_set && restarts == 0)
# and back on for the goals, so the forcing does not keep the update remaking
# gen.mk for ever.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk: helper
	@printf 'GENERATED := from-generated\n' > $@

helper:
	@printf 'h\n' > helper
	@$(MAKE) -C sub marker
