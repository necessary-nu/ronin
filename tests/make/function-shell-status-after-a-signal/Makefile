# A signal is not an exit code. GNU Make reports one the way a shell reports a
# child of its own, and a command that was never there is the shell's 127.
K := $(shell kill -TERM $$$$)
SIGNALLED := $(.SHELLSTATUS)
M := $(shell no-such-command-anywhere 2>/dev/null)
MISSING := $(.SHELLSTATUS)
all: ; @printf '%s\n' 'K=[$(K)]' 'SIGNALLED=[$(SIGNALLED)]' 'MISSING=[$(MISSING)]' > out
