# GNU expands a pattern-specific variable name once for every target pattern,
# while each expansion still occurs in the ambient rather than pattern scope.
name = $(shell printf 'n\n' >> count; printf N)
%.a %.b: $(name)=good

.PHONY: all
all: ; @cp count result
