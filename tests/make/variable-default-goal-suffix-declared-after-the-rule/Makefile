# The check reads `.SUFFIXES` as it stands at the moment each target is
# recorded, so a declaration below the rule has not happened yet and does not
# reject it.
foo: ; @printf '%s\n' 'foo ran' > out
.SUFFIXES: foo bar
zed: ; @printf '%s\n' 'zed ran' > out
