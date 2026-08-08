all: a.out
F = global
%.out: private F = pattern
%.out: dep ; @printf '[%s]\n' '$(F)' > $@
dep: ; @printf '[%s]\n' '$(F)' > dep.txt
all: ; @printf '[%s]\n' '$(F)' > all.txt
