# The other half of the same premise. These two makefiles WERE part of the first
# read, and the first child rewrites one and removes the other before the
# staging pass re-reads this text. GNU Make read them once and kept what it
# read, so both answers below are the text as it stood when the build started.
include kept.mk
-include gone.mk
all:
	@$(MAKE) --no-print-directory -f first.mk sub
	@$(MAKE) --no-print-directory -f second.mk sub
	@printf 'K=[$(K)] G=[$(G)]\n' > answers
