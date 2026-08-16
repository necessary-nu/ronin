# The failure need not be in the chain entry itself. Here the first entry's own
# prerequisite fails, so that entry never runs at all — and under `-k` the
# target's second entry still does. A chain entry waits for the one before it
# to finish rather than to succeed, and an entry that could never start counts
# as finished for that purpose.
all: out
	@printf '%s\n' dependent >> log

out:: p1
	@printf '%s\n' first >> log
out:: n2
	@printf '%s\n' second >> log

p1:
	@printf '%s\n' prerequisite >> log
	@false
