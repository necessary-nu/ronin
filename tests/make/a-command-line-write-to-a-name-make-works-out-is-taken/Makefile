# The command line is the second way a write reaches a name GNU Make works out
# for itself, and it is refused there no more than a makefile line is. The
# argv word for `.VARIABLES` is stored and never read back; the one for
# `.SHELLSTATUS` defines the name, because no `$(shell)` has run to claim it.
all:
	@printf 'FOO=[%s] status=[%s] origin=[%s]\n' '$(FOO)' '$(.SHELLSTATUS)' '$(origin .SHELLSTATUS)' > out
	@printf 'wrote=[%s]\n' '$(filter from-argv,$(.VARIABLES))' >> out
