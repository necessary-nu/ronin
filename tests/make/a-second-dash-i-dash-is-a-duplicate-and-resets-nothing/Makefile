# The second `-I -` is filtered out as a duplicate of the first, so it never
# reaches the search path and resets nothing: the directory named between them
# stands, with the built-in default path still off behind it.
-include frag.mk

ifeq ($(FOUND),yes)
all: the-directory-stood
else
all: the-directory-was-forgotten-again
endif

the-directory-stood: ; @printf 'found=[%s] dirs=[%s]\n' '$(FOUND)' '$(.INCLUDE_DIRS)' > out
the-directory-was-forgotten-again: ; @printf 'dirs=[%s]\n' '$(.INCLUDE_DIRS)' > out
