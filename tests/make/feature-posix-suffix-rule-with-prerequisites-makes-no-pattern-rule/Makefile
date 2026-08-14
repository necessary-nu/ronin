# POSIX says a suffix rule has no prerequisites. Under `.POSIX:` GNU Make
# passes the pair over rather than converting it and dropping them, so no
# `%.biz: %.baz` rule exists and `foo.biz` cannot be made — though `foo.baz`
# is right there.
.POSIX:
.SUFFIXES:
.SUFFIXES: .biz .baz

.baz.biz: foo.bar ; @printf '%s\n' 'made $@ from $<' > out
all: foo.biz
