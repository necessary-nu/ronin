# `-` is an entry in the switch table's `-I` list rather than a state the table
# remembers, and the duplicate filter runs before `construct_include_path` ever
# sees it. So the second `inc` matches the first and is dropped, and the reset in
# between has nothing left after it: the search path is empty and the fragment is
# unreachable through a directory named twice.
-include frag.mk

ifeq ($(FOUND),yes)
all: the-directory-came-back
else
all: the-directory-stayed-forgotten
endif

the-directory-came-back: ; @printf 'found=[%s] dirs=[%s]\n' '$(FOUND)' '$(.INCLUDE_DIRS)' > out
the-directory-stayed-forgotten: ; @printf 'dirs=[%s]\n' '$(.INCLUDE_DIRS)' > out
