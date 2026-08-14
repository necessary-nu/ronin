# A rule whose target has the shape of a suffix pair names a file as well as
# defining a suffix rule. GNU Make converts the pair into `%.biz: %.baz` and
# leaves the `.baz.biz` entry in the file database, so asking for it by name
# runs this recipe against the prerequisite as written.
.SUFFIXES:
.SUFFIXES: .biz .baz

.baz.biz: foo.bar ; @printf '%s\n' 'made $@ from $<' > out
