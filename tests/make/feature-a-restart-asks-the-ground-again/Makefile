# A restart is not a staging pass. The text is new — that is why the read
# starts over — so GNU Make reads it again and asks everything again, and
# `MAKE_RESTARTS` exists precisely so a Makefile can tell which time it is.
# The first read's answers belong to the first read's text and must not be
# handed to this one, so the `$(shell)` runs once per pass and appends twice.
$(shell printf 'asked\n' >> shelllog)
all: gen.mk
	@printf 'V=[$(V)] lines=[$(words $(file < shelllog))]\n' > answers
include gen.mk
gen.mk: ; @printf 'V := built\n' > gen.mk
