# `else ifeq` and `else ifdef` are one directive, not a nested conditional, so
# a chain of them needs exactly one endif.
a = first
b = second

ifeq ($(a),$(b))
answer = first-leg
else ifneq ($(a),$(b))
answer = second-leg
else
answer = last-leg
endif

ifdef missing
other = defined
else ifdef a
other = else-ifdef
else
other = fell-through
endif

all: ; @printf '%s %s\n' '$(answer)' '$(other)' > out
