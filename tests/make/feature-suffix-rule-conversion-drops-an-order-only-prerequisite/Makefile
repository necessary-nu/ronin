# `f->deps` is the whole prerequisite chain, so an order-only prerequisite
# disqualifies the pair from POSIX conversion and is dropped by the ordinary
# one exactly as a normal prerequisite is. It must not survive into the
# pattern rule, where the stem would be substituted into it.
.SUFFIXES:
.SUFFIXES: .biz .baz

.baz.biz: | foo.bar ; @printf '%s\n' 'made $@ from $<' > out
