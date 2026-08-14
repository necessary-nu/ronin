# Every environment GNU Make builds carries MAKELEVEL one deeper than its own,
# and takes the depth from the invocation rather than from the variable, so a
# makefile that assigns MAKELEVEL changes what it reads and not what its
# children are told.
MAKELEVEL = 9
$(file >shell-out,[$(shell echo $$MAKELEVEL)])
all: ; @printf 'recipe=[%s] make=[%s]\n' "$$MAKELEVEL" "$(MAKELEVEL)" > out
