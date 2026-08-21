# The same under -t, where the difference is visible in the bytes: GNU Makes
# child RUNS its recipe, so marker.out holds what the recipe wrote. A child
# that was handed -t would touch the file instead and leave it empty.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@; $(MAKE) --no-print-directory -C sub marker
