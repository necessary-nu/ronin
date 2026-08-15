V = outer
W = outer
%.p: V = early-public
a.%: private V = late-private
%.q: private W = early-private
b.%: W = late-public
all: a.p b.q
a.p: dep.a ; @printf 'V=[%s]\n' '$(V)' > ap.out
dep.a: ; @printf 'V=[%s]\n' '$(V)' > depa.out
b.q: dep.b ; @printf 'W=[%s]\n' '$(W)' > bq.out
dep.b: ; @printf 'W=[%s]\n' '$(W)' > depb.out
