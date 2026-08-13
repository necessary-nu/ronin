# Two patterns of the same length both match, so neither is more specific than
# the other; the one written first is applied first, and the later one wins.
V = global
W = global

a%c: V += first
%bc: V += third

%bd: W += third
a%d: W += first

all: abc abd
abc abd: ; @printf 'V=%s W=%s\n' '$(V)' '$(W)' > $@
