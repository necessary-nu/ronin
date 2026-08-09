# An override assignment wins against the command line, and the two flavours
# still differ: the recursive one reads X where it is used, the simple one
# where it was written.
X = start
override recur = $(X)
override simple := $(X)
X = end

all: ; @printf '%s %s\n' '$(recur)' '$(simple)' > out
