# -t is off across the makefile update for the reason -n is: a Makefile dated
# and not written is one the read cannot use. gen.mk is written for real and the
# child runs, and then all is touched rather than made.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@
	@$(MAKE) -C sub marker
