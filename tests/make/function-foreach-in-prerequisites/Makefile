names := a b c
all: $(foreach n,$(names),$(n).txt) ; @printf '[%s]\n' '$^' > out
%.txt: ; @printf '%s\n' '$@' > $@
