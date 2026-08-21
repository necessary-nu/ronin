# The same makefile asked for as the goal. The read still has to remake it —
# it is a makefile whatever the command line wants — and the goal is then a
# file that is already up to date, so the recursion runs once and only once.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@
	@$(MAKE) -C sub marker
