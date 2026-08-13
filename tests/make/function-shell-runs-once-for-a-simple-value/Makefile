# A simple variable's $(shell) runs once, while a recursive one's runs at every
# expansion, so the tick file counts the expansions rather than the writes.
SIMPLE := $(shell echo s >> tick; echo simple)
RECURSIVE = $(shell echo r >> tick; echo recursive)
all: ; @printf '%s %s %s\n' '$(SIMPLE)' '$(RECURSIVE)' '$(RECURSIVE)' > out; cp tick ticks
