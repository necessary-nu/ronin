# A group only opens when some later word ends in `)`. GNU Make looks ahead for
# one before it reopens the parenthesis, and with none there the text is not a
# group at all — so this names a file literally called `libx.a(a.o`, and `b.o`
# beside it, rather than two members.
all: libx.a(a.o b.o
	@echo "[$^]" > log
