# `func_file` ends a read with
# `if (n && o[-1] == '\n') o -= 1 + (n > 1 && o[-2] == '\r');`
# — one trailing newline goes and no more, so what is removed is the line
# terminator the last line carries rather than the blank lines before it.
#
# The newlines that survive are shown as `<NL>`, because a recipe splits on a
# real one. The carriage-return half of that rule is asserted byte-for-byte in
# kati's own tests instead: this harness records a file through `str::lines`,
# which drops a `\r` before the newline and so cannot see it.
define newline


endef
show = printf '%s=[%s]\n' '$(1)' '$(subst $(newline),<NL>,$(file < $(1)))'

all:
	@$(call show,one-newline) > out
	@$(call show,two-newlines) >> out
	@$(call show,three-newlines) >> out
	@$(call show,no-newline) >> out
	@$(call show,only-newline) >> out
	@$(call show,empty) >> out
	@$(call show,absent) >> out
