# A literal `=` after an earlier prerequisite is not an assignment operator:
# the text between the colon and the operator is more than one word. Both
# prerequisites and the goal's own recipe therefore have to run.
NAME = name=value

.PHONY: all first $(NAME)

all: first name=value ; @printf '%s\n' goal > out

first: ; @printf '%s\n' first > first-out

$(NAME): ; @printf '%s\n' equals > equals-out
