# -q builds the Makefiles for real and then answers about the goals. Here the
# answer is yes: once gen.mk has been made, the goal that wanted it is current.
all: gen.mk

include gen.mk

gen.mk: ; @printf 'V := built\n' > $@
