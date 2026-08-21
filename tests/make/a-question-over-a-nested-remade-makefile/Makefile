# The same under -q. The makefile update is not a question, so gen.mk is made
# and the child runs; the question is asked of the goal alone.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@; $(MAKE) --no-print-directory -C sub marker
