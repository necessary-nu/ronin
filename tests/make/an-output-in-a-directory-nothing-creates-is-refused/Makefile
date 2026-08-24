# Nothing creates `sub/`, so the recipe's redirection has nowhere to write and
# the shell fails. GNU Make reports `Error 2` and the goal is not made. A tool
# that made the directory itself would succeed here and write a file GNU Make
# does not.
all: sub/out
sub/out: ; @echo made > $@
