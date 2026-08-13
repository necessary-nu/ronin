# The Makefile the invocation named is remade like any other one it reads. When
# its own rule rewrites it, the read starts again and the goals are the ones the
# new text describes.
all: ; @printf 'restarts=%s tag=%s\n' '$(MAKE_RESTARTS)' '$(TAG)' > out

TAG := original

Makefile: Makefile.in ; @sed 's/original/regenerated/' $< > $@
