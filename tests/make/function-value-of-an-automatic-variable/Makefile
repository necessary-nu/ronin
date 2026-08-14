# GNU Make sets the base automatic variables per file, in `set_file_variables`,
# as simple variables whose value is the name it just computed. A simple
# variable has no unexpanded text behind it, so reading one back with `value`
# yields the same text that expanding it does: `$(value @)` is `$@`.
.PHONY: all ord
all: a.in b.in | ord
	@printf '%s\n' 'at [$(value @)]' 'lt [$(value <)]' 'hat [$(value ^)]' \
		'plus [$(value +)]' 'star [$(value *)]' 'q [$(value ?)]' \
		'bar [$(value |)]' 'expand at [$@]' 'expand hat [$^]' > out

a.in b.in: ; @: > $@

ord: ; @:
