# The pattern rule the conversion produces reads only `%.baz`: the `foo.bar`
# written here is not one of its prerequisites, so a `foo.biz` newer than
# `foo.baz` is current however new `foo.bar` is.
.SUFFIXES:
.SUFFIXES: .biz .baz

.baz.biz: foo.bar ; @printf '%s\n' 'remade $@' > out
