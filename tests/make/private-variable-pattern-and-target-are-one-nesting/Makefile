V = outer
W = outer
%.p: private V = pattern-private
q.p: V = target-public
%.r: W = pattern-public
q.r: private W = target-private
all: q.p q.r
q.p: dep-p ; @printf 'V=[%s]\n' '$(V)' > qp.out
dep-p: ; @printf 'V=[%s]\n' '$(V)' > depp.out
q.r: dep-r ; @printf 'W=[%s]\n' '$(W)' > qr.out
dep-r: ; @printf 'W=[%s]\n' '$(W)' > depr.out
