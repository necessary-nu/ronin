# The switch table filters a duplicate out of every list switch but `-f`, and it
# compares the word as WRITTEN against the names already stored — which have been
# through `expand_command_line_file`. So `-I ./inc` stores `inc`, and the `-I inc`
# behind it matches that and is dropped: one directory, named twice.
include frag.mk

ifeq ($(words $(filter inc ./inc,$(.INCLUDE_DIRS))),1)
all: the-directory-was-filtered
else
all: the-directory-was-listed-twice
endif

the-directory-was-filtered: ; @printf 'found=[%s] named=[%s]\n' '$(FOUND)' '$(filter inc ./inc,$(.INCLUDE_DIRS))' > out
the-directory-was-listed-twice: ; @printf 'repeated=[%s]\n' '$(filter inc ./inc,$(.INCLUDE_DIRS))' > out
