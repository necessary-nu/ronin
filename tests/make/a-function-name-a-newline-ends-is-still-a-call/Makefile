# The name terminator GNU Make accepts is any whitespace, so a newline ends a
# name as a tab and a space do. A `define` body is where a call can meet one:
# the text it holds spans lines, and expanding it walks the newline exactly as
# it walks a space.
define TAIL
$(subst
a,b,aaa)
endef

all: ; @echo [$(TAIL)] > out
