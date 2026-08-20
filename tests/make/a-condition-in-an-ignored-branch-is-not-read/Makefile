# GNU Make decides it is ignoring BEFORE it looks at an `if...' condition:
# `conditional_line' returns from its ignoring loop above both the `ifdef' arm
# and the `ifeq' arm. So a condition inside a branch that is not being taken is
# never split, never expanded and never complained about -- whatever it says.
ifeq (x,y)

# The close never arrives, in each of the spellings that reach the same arm.
ifeq (a,a junk
UNREAD := from-the-unread-branch
endif

ifneq (b,b junk
endif

ifeq "c" c
endif

ifeq junk
endif

ifeq ($(subst a,b,aaa,bbb)
endif

# It is still COUNTED, though, which is the other half: the `endif' above this
# line closes the conditional above it and not the one this branch belongs to,
# so this assignment is inside the ignored branch and never happens.
INSIDE := from-the-outer-branch
endif

AFTER := after-the-outer-endif

all: ; @printf 'inside=[$(INSIDE)] unread=[$(UNREAD)] after=[$(AFTER)]\n' > out
