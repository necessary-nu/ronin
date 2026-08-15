# A backslash escapes the wildcard, so this rule has two plain targets and
# nothing is mixed. The escaping is compressed out of the name it records.
all: ; @printf top > out
b\%c d: ; @printf 'ran %s\n' '$@' > $@
