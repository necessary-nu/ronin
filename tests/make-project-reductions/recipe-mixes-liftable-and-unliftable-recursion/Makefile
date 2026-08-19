# vim's top-level Makefile in miniature. GNU runs the guard, `false` is false,
# and the only recursion that happens is the one into src/. Ronin refuses the
# whole recipe because one line holds a $(MAKE) it cannot lift out statically.
all:
	cd src && $(MAKE) all
	@if false; then (cd x && $(MAKE) z); fi
