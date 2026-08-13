# What restarts the read is a Makefile that was written, not a rule that ran.
# This recipe fires because its prerequisite is out of date and then puts the
# file back exactly as it found it, so the text the read already has is still
# the text, and it is not read again.
all: ; @printf 'restarts=%s\n' '$(MAKE_RESTARTS)' > out

include gen.mk

gen.mk: gen.in ; @printf 'ran\n' >> log; touch -d '2020-01-01 00:00:00' $@
