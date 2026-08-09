.SUFFIXES:
.SUFFIXES: .in .mid .out
.PRECIOUS: hello.mid
all: hello.out
hello.in: ; echo body > $@
.in.mid: ; cat $< > $@
.mid.out: ; cat $< > $@
