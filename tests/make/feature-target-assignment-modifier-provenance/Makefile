# Modifier words are recognized in the same hybrid text as the assignment:
# entirely inside the colon-bearing expansion, entirely in the literal suffix,
# or split across those two sides of the boundary.
WHOLE_RULE = whole: private WHOLE_VALUE=good
COLON = :
MIXED_HEAD = mixed: private

$(WHOLE_RULE)
suffix$(COLON) private SUFFIX_VALUE=value
$(MIXED_HEAD) MIXED_VALUE=both

.PHONY: all whole suffix mixed
all: whole suffix mixed ; @printf '%s\n' all > all-out
whole: ; @printf '<%s>\n' '$(WHOLE_VALUE)' > whole-out
suffix: ; @printf '<%s>\n' '$(SUFFIX_VALUE)' > suffix-out
mixed: ; @printf '<%s>\n' '$(MIXED_VALUE)' > mixed-out
