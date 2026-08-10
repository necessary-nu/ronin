# An explicit rule expands its retained prerequisite list as a whole, so the
# trailing backslash in one raw word can quote the blank before the next one.
.SECONDEXPANSION:
S = b

.PHONY: all
all: a\ $$(S)

a\ b: ; @printf '<%s>\n' '$@' > result
