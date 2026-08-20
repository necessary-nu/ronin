# Two environment names are not simply taken as they arrived, and neither of
# them becomes an expression when the invocation puts a `$` in it.
#
# MAKELEVEL is defined over again once the environment is in scope —
# `define_variable_cname (MAKELEVEL_NAME, buf, o_env, 0)` in
# `define_automatic_variables`, writing back the DEPTH GNU Make parsed with the
# recursive flag OFF. So the value is a number that did not parse, and the
# flavour is simple: the one imported name whose `$` is never read.
#
# SHELL is not imported at all — POSIX says a makefile's SHELL must not change
# the one subprocesses are given, so the import is withheld and the built-in
# stands. What `$(SHELL)` answers therefore has nothing to do with what the
# environment carried, expression or not.
$(file > out,level=[$(MAKELEVEL)] flavor=[$(flavor MAKELEVEL)] origin=[$(origin MAKELEVEL)])
$(file >> out,shell=[$(SHELL)] flavor=[$(flavor SHELL)] origin=[$(origin SHELL)])

all:
	@printf 'ran\n' > ran
