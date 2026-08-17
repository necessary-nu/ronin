# The other half of the same conversion. One `.c.a:` rule makes `(%.o): %.c`
# and `%.a: %.c`, and this is the second — an archive named as an ordinary
# target, made from the source of that name. `$%` is empty because the target
# is not a member of anything.
.SUFFIXES:
.SUFFIXES: .a .o .c
.c.a:
	@echo "at=$@ pct=$% star=$* lt=$<" >> log
all: libz.a
