# Parentheses produced by expansion do not quote a later semicolon. It starts
# the inline recipe after the ordinary expansion/unquoting pass.
OPEN = (
TAIL = $(OPEN); @printf '<%s>\n' '$@' > all-out

.PHONY: all
all: $(TAIL)
$(OPEN): ; @printf '<%s>\n' '$@' > open-out
