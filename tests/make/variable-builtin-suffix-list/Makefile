# `SUFFIXES` is the readable half of the list `.SUFFIXES` carries. It is a
# simple variable at `default` origin, which is the weakest there is, and `-r`
# leaves the name bound and empty rather than undefined.
all: ; @printf '[%s]\n[%s] [%s]\n' '$(SUFFIXES)' '$(origin SUFFIXES)' '$(flavor SUFFIXES)' > out
