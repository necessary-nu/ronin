# Backslash quotes the blank inside a rule target and compacts away before the
# file-name sequence is split. Escaped colons undergo their earlier unquote.
.PHONY: all
all: a\ b

a\ b: a\:b ; @printf '<%s> <%s>\n' '$@' '$<' > result
a\:b: ; @:
