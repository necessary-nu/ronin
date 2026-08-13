.SUFFIXES:
.SUFFIXES: .in .mid .out
.PRECIOUS: %.mid
all: hello.out
hello.in: ; echo body > $@
.in.mid: ; cat $< > $@
.mid.out: ; cat $< > $@
