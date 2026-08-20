# The `else if...' spelling reaches the same arm by a different road. GNU Make's
# `else' handling calls `conditional_line' again for the text after it, and that
# call finds the level below already taken and returns before reading a byte of
# the condition.
ifeq (x,x)
TAKEN := first
else ifeq (y,y junk
UNREAD := second
else
UNREAD := third
endif

# The same thing further down a chain: the arm that cannot be read sits below
# the arm that was taken, so it is never read either.
ifeq (p,q)
CHAIN := first
else ifeq (r,r)
CHAIN := second
else ifeq (s,s junk
CHAIN := third
endif

all: ; @printf 'taken=[$(TAKEN)] unread=[$(UNREAD)] chain=[$(CHAIN)]\n' > out
