# A concrete multi-target assignment expands its variable name separately in
# each target's existing file scope.
a: N=A
b: N=B
a b: $(N)=good

.PHONY: all a b
all: a b ; @printf '%s\n' all > all-out
a: ; @printf '<%s>|<%s>\n' '$(A)' '$(B)' > a-out
b: ; @printf '<%s>|<%s>\n' '$(A)' '$(B)' > b-out
