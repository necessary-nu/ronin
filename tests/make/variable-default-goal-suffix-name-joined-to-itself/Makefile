# The two loops range over the same list, so a suffix joined to itself rejects
# a target of that name too.
.SUFFIXES:
.SUFFIXES: foo
foofoo: ; @printf '%s\n' 'foofoo ran' > out
zed:    ; @printf '%s\n' 'zed ran'    > out
