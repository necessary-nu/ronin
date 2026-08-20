-include frag.mk
all: ; @printf 'child found=[%s] named=[%s]\n' '$(FOUND)' '$(firstword $(.INCLUDE_DIRS))' > out
