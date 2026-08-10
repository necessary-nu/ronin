# Once assignment classification fails, GNU unquotes an equals sign in the
# untouched source suffix. The already-expanded colon word is the control: its
# quoting backslash remains.
NAME = foo=bar
QUOTED_NAME = foo\=bar
EXPANDED_RULE = expanded: first foo\=bar

.PHONY: all expanded literal first
all: expanded literal ; @printf '%s\n' all > all-out
$(EXPANDED_RULE) ; @printf '%s\n' expanded > expanded-out
literal: first foo\=bar ; @printf '%s\n' literal > literal-out
first: ; @printf '%s\n' first > first-out
$(NAME): ; @printf '%s\n' equals > equals-out
$(QUOTED_NAME): ; @printf '%s\n' quoted > quoted-out
