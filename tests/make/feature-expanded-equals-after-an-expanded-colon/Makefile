# GNU Make expands source words only through the one that produces the rule's
# colon before deciding whether the rest is a target-specific assignment. The
# next source word is still literal then, so its expanded `=` is a prerequisite
# character even though the colon was also produced by expansion.
COLON = :
EQ = =

all$(COLON) one$(EQ)two ; @printf '%s\n' goal > out

one$(EQ)two: ; @printf '%s\n' prerequisite > dep-out
