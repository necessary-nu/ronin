# `libx.a(a.o b.o)` is one word to the tokenizer and two names to GNU Make.
# `parse_file_seq` (reference/gnumake/src/read.c) keeps the text up to the `(`
# as a prefix and closes a pair around each following word, which is why this
# spelling and `libx.a(a.o) libx.a(b.o)` mean the same thing.
all: libx.a(a.o b.o)
	@echo "[$^]" > log
