# A bare `.SUFFIXES:` clears the list, so a name that would have been rejected
# by an earlier declaration is admitted again.
.SUFFIXES: foo bar
.SUFFIXES:
foo: ; @printf '%s\n' 'foo ran' > out
zed: ; @printf '%s\n' 'zed ran' > out
