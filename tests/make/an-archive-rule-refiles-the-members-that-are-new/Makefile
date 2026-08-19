# GNU Make's own SV 61436 case, "Allow redefining archive rules to propagate
# timestamps": the archive is filed by a rule of the makefile's own, and what it
# files is `$?` — the members that are new. The value is decided after the
# prerequisites have been made, so it reaches the recipe as text the runner
# substitutes, and an archive member's name has parentheses in it: a value spelt
# `mylib.a(a.o)` reaches the shell as a syntax error rather than as two objects
# to file.
mylib.a: mylib.a(a.o b.o)

(%): % ;

%.a: ; $(AR) $(ARFLAGS) $@ $?

%.o : %.c
	@printf '%s\n' 'compile $<' >> log
	@cp $< $@
