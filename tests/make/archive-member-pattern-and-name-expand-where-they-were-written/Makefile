# Globbing and listing are the same mechanism seen twice: each word inside the
# parentheses is expanded on its own, in the place it was written. `c.o` is a
# name and stays first; `*.o` is a pattern and contributes its own matches,
# sorted, after it — including `c.o` a second time.
all: libx.a(c.o *.o)
	@echo "[$^]" > log
