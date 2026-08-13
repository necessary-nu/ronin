# The question is whether the file was written, not whether writing it changed
# anything a reader would notice. This recipe writes back the bytes that were
# already there, and the read still starts again.
all: ; @printf 'restarts=%s value=%s\n' '$(MAKE_RESTARTS)' '$(V)' > out

include gen.mk

gen.mk: gen.in ; @printf 'V := same\n' > $@
