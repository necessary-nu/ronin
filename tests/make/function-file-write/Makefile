define A
a
b
endef
B = c d
$(file >file.out,$(A))
$(foreach L,$(B),$(file >>     file.out,$(L)))
all: ; @printf '%s\n' 'done' > out
