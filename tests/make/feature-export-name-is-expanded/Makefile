# The names on an export line are expanded first, so which variable is exported
# can be computed.
hello = world
NAME = hello

export $(NAME)

all: ; @printf 'hello=%s\n' "$$hello" > out
