# `-f` is the one list switch GNU Make does not filter — `Allow duplicate
# makefiles for backward compatibility` — so naming the same file twice reads it
# twice. Both spellings canonicalise to the same name, which is what makes the
# two entries a repeat rather than two files.
N += x

all: ; @printf 'n=[%s] list=[%s]\n' '$(N)' '$(MAKEFILE_LIST)' > out
