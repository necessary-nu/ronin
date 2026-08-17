# The two rules a `.c.a:` makes are installed independently, each with
# `new_pattern_rule`'s override left off. So a makefile that writes `%.a: %.c`
# by hand takes the name of the conversion's second half and leaves the first
# standing: the archive-member rule still comes from `.c.a:`.
#
# Recorded because the obvious implementation loses it — deriving `(%.o): %.c`
# from whichever suffix rules survived the written-rule comparison finds
# nothing here, the written rule having already claimed the entry.
.SUFFIXES:
.SUFFIXES: .a .o .c
.c.a:
	@echo "suffix at=$@ pct=$%" >> log
%.a: %.c
	@echo "written at=$@" >> log
all: libx.a(foo.o)
