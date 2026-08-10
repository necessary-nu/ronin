# A `#` produced while expanding the colon-bearing word still stops GNU Make's
# variable-definition scanner.  This is a normal rule with a missing
# prerequisite, not a target-specific assignment to `FOO#BAR`.
HASH := \#
RULE := all:FOO$(HASH)BAR=good
$(RULE)
