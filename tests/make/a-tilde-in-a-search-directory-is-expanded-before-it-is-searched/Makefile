# A `-I` directory beginning with `~` is expanded before it is stat'ed
# (`tilde_expand`, read.c), so `-I ~` searches the home directory rather than a
# directory literally called `~` — which is not there, and would drop out of the
# path unnoticed. The fragment is only reachable through the expansion, so
# finding it is the assertion.
-include frag.mk

ifeq ($(FOUND),yes)
all: the-tilde-was-expanded
else
all: the-tilde-stood-as-written
endif

the-tilde-was-expanded: ; @printf 'found=[%s] named=[%s]\n' '$(FOUND)' '$(filter inc,$(.INCLUDE_DIRS))' > out
the-tilde-stood-as-written: ; @printf 'branch=[%s]\n' '$(.INCLUDE_DIRS)' > out
