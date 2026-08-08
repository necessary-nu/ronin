P = global
Q = global
all: private P = mine
all: Q = ours
all: dep
dep: ; @printf 'P=[%s] Q=[%s]\n' '$(P)' '$(Q)' > dep.out
all: ; @printf 'P=[%s] Q=[%s]\n' '$(P)' '$(Q)' > all.out
