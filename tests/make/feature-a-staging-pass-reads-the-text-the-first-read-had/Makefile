# Two recursive children make the compiler read this text again once the first
# child's work is staged, and by then that child has written `gen.mk' and
# `parts/one.mk'. GNU Make reads once, before any recipe runs, so its read never
# saw either file exist -- and nothing regenerates them, so there is no restart
# in which it ever would.
#
# Which files a read opens is not a value handed to an expansion; it is what the
# read IS. A repeat that opened these would be compiling a makefile the build
# never had, so every answer below must be the first read's: `G' and `P' from
# text that was never included, `S' from a `$(shell)' that must not run, and
# `shelllog' holding the one line the parent's own `$(shell)' put there.
-include gen.mk
-include parts/*.mk
X := $(shell printf 'asked\n' >> shelllog; echo told)
all:
	@$(MAKE) --no-print-directory -f first.mk sub
	@$(MAKE) --no-print-directory -f second.mk sub
	@printf 'X=[$(X)] G=[$(G)] S=[$(S)] P=[$(P)]\n' > answers
