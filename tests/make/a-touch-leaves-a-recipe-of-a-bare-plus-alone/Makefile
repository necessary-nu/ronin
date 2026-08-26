# GNU Make's one carve-out from the touch, on a recipe that also comes to no
# command: every written line is recursive, so `-t` runs them rather than
# standing in for them, and `b` is left exactly as it was found. The
# classification is read off the line AS WRITTEN, which is why a bare `+` is one
# and the `$(NOTHING)` beside it is not.
.SUFFIXES:
a: b
	@echo made > $@
b: c
	+
