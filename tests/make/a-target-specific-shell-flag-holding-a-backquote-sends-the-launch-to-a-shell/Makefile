# `$(.SHELLFLAGS)` is read per launch with the target's own scope, so a value
# one target sets reaches the same launch decision as a global one. Where the
# flags a target sets hold shell syntax, that target's line goes to a shell to
# split them, exactly as a global `.SHELLFLAGS` would — the launch decision is
# about the value, not about where the value came from.
#
# THE GATE IS THE FILE `out`, written because the line reached a shell that
# dropped the empty field the backquote expanded to rather than an argv that
# kept it.
SHELL := /bin/sh
all: .SHELLFLAGS := -c `>pwned`
all:
	@echo one > out
