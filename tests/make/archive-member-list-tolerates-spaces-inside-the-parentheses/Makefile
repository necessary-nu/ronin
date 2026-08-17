# A word that is only the opening `libx.a(`, or only the closing `)`, separates
# rather than names: GNU Make skips it and the group carries on. So the spacing
# a makefile uses inside the parentheses says nothing about which members it
# asked for.
all: libx.a(  a.o    b.o   )
	@echo "[$^]" > log
