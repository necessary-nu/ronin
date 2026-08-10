# find_char_unquote compacts an even backslash run before an expanded
# separator. The expanded command is parsed again at recipe time, so the
# dollar preserved by the first expansion still resolves VAR.
VAR = good
RULE = all: slash\\; @printf '<%s>\n' '$$(VAR)' > command-out
PREREQUISITE = slash\\:

$(RULE)
$(PREREQUISITE) ; @printf '<%s>\n' '$@' > prerequisite-out
