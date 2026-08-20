# GNU Make reads every non-switch word of a `MAKEFLAGS` write through
# `handle_non_switch_argument`, whatever origin the write arrived from, and
# `try_variable_definition` then defines it at that origin — `o_file` for a
# Makefile's own write. So a word that names something is an ordinary Makefile
# assignment made where the write stands: it speaks over what is above it, a
# later line speaks over it, `$(origin)` answers `file`, and the whole operator
# grammar comes along. Nothing of it travels: `MAKEOVERRIDES` was rendered
# before the first makefile was read.
BEFORE = mine
GROWN = head
MAKEFLAGS += BEFORE=written GROWN+=tail SETTLED:=$(BEFORE) AFTER=written
AFTER = mine
all:
	@printf 'BEFORE=[%s] origin=[%s]\n' '$(BEFORE)' '$(origin BEFORE)' > out
	@printf 'GROWN=[%s] SETTLED=[%s] AFTER=[%s]\n' '$(GROWN)' '$(SETTLED)' '$(AFTER)' >> out
	@printf 'MAKEFLAGS=[%s] MAKEOVERRIDES=[%s]\n' '$(MAKEFLAGS)' '$(MAKEOVERRIDES)' >> out
