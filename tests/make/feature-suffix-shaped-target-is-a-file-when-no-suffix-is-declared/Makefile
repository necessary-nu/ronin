# Being suffix-shaped is not what makes a name a suffix rule: the pair has to
# be on `.SUFFIXES` for the conversion to reach it. With neither half declared
# there is no rule to convert, and `.baz.biz` is an ordinary file target.
.SUFFIXES:
.SUFFIXES: .zip .zap

.baz.biz: ; @printf '%s\n' 'ordinary $@' > out
