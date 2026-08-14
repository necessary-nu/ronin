# The stem is an automatic variable like any other, so the same split holds
# inside an implicit rule: `$(value *)` reads back the stem the match produced,
# while `$(value *D)` reads back the expression its `D` form was defined from.
.PHONY: all
all: sub/thing.o

sub/%.o: %.in
	@mkdir -p $(@D)
	@printf '%s\n' 'at [$(value @)]' 'lt [$(value <)]' 'star [$(value *)]' \
		'starD [$(value *D)]' 'starF [$(value *F)]' 'expand star [$*]' > out
	@: > $@

thing.in: ; @: > $@
