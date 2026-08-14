# The same shape with prerequisites is a cancellation instead, and a cancelled
# rule is passed over before it can count as a match at all — so the
# match-anything rule survives and makes `foo.tex`.
%: ; @printf 'anymatch\n' > $@

%.tex: %.w

all: foo.tex
