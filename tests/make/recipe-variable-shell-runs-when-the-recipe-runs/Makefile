# A recursively expanded variable holding a shell call is expanded where the
# recipe referencing it is expanded, which is after the prerequisite that
# writes the file the call looks for.
PROBE = $(shell test -f marker && printf 'found-%s' "`cat marker`")

all: marker
	@printf 'probe says: [%s]\n' '$(PROBE)' > out

marker:
	@printf 'yes\n' > marker
.PHONY: all
